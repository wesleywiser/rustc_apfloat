use afl::fuzz;
use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    /// Erase NaN payloads from hardware floating-point operations
    #[arg(long)]
    erase_hard_nans: bool,

    /// Compare with C++ (LLVM's original) APFloat, not just host hardware
    #[arg(long)]
    cxx: bool,

    /// HACK(eddyb) ignore hardware (for testing)
    #[arg(long)]
    ignore_hard: bool,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum FuzzReq<T> {
    Neg(T) = 0,

    // FIXME(eddyb) deduplicate this mess (auto-generate both C++ & Rust!).
    Add(T, T) = 1,
    Sub(T, T) = 2,
    Mul(T, T) = 3,
    Div(T, T) = 4,
    Rem(T, T) = 5,

    MulAdd(T, T, T) = 6,
}

impl FuzzReq<()> {
    fn from_op(op: u8) -> Option<Self> {
        Some(match op {
            0 => FuzzReq::Neg(()),
            1 => FuzzReq::Add((), ()),
            2 => FuzzReq::Sub((), ()),
            3 => FuzzReq::Mul((), ()),
            4 => FuzzReq::Div((), ()),
            5 => FuzzReq::Rem((), ()),
            6 => FuzzReq::MulAdd((), (), ()),
            _ => return None,
        })
    }
}

impl<T> FuzzReq<T> {
    fn map<U>(self, mut f: impl FnMut(T) -> U) -> FuzzReq<U> {
        match self {
            FuzzReq::Neg(x) => FuzzReq::Neg(f(x)),
            FuzzReq::Add(a, b) => FuzzReq::Add(f(a), f(b)),
            FuzzReq::Sub(a, b) => FuzzReq::Sub(f(a), f(b)),
            FuzzReq::Mul(a, b) => FuzzReq::Mul(f(a), f(b)),
            FuzzReq::Div(a, b) => FuzzReq::Div(f(a), f(b)),
            FuzzReq::Rem(a, b) => FuzzReq::Rem(f(a), f(b)),
            FuzzReq::MulAdd(a, b, c) => FuzzReq::MulAdd(f(a), f(b), f(c)),
        }
    }
}

trait HardFloat: num_traits::Float {
    type RsApF: rustc_apfloat::Float;
    type Bits: Default + num_traits::PrimInt + num_traits::Unsigned;
    fn bits_from_le_bytes(bytes: &[u8]) -> Self::Bits;
    fn from_bits(uint: Self::Bits) -> Self;
    fn to_bits(self) -> Self::Bits;

    // FIXME(eddyb) this is a silly place to put this, rethink the trait!
    fn cxx_apf_eval_fuzz_req(req: FuzzReq<Self::Bits>) -> Self::Bits;
}

impl HardFloat for f32 {
    type RsApF = rustc_apfloat::ieee::Single;
    type Bits = u32;
    fn bits_from_le_bytes(bytes: &[u8]) -> Self::Bits {
        u32::from_le_bytes(*<&[u8; 4]>::try_from(bytes).unwrap())
    }
    fn from_bits(uint: Self::Bits) -> Self {
        Self::from_bits(uint)
    }
    fn to_bits(self) -> Self::Bits {
        self.to_bits()
    }

    fn cxx_apf_eval_fuzz_req(req: FuzzReq<Self::Bits>) -> Self::Bits {
        extern "C" {
            fn cxx_apf_fuzz_eval_req_ieee32(req: FuzzReq<u32>) -> u32;
        }
        unsafe { cxx_apf_fuzz_eval_req_ieee32(req) }
    }
}

impl HardFloat for f64 {
    type RsApF = rustc_apfloat::ieee::Double;
    type Bits = u64;
    fn bits_from_le_bytes(bytes: &[u8]) -> Self::Bits {
        u64::from_le_bytes(*<&[u8; 8]>::try_from(bytes).unwrap())
    }
    fn from_bits(uint: Self::Bits) -> Self {
        Self::from_bits(uint)
    }
    fn to_bits(self) -> Self::Bits {
        self.to_bits()
    }

    fn cxx_apf_eval_fuzz_req(req: FuzzReq<Self::Bits>) -> Self::Bits {
        extern "C" {
            fn cxx_apf_fuzz_eval_req_ieee64(req: FuzzReq<u64>) -> u64;
        }
        unsafe { cxx_apf_fuzz_eval_req_ieee64(req) }
    }
}

impl<HF: HardFloat> FuzzReq<HF> {
    fn eval_hard(self) -> HF {
        match self {
            FuzzReq::Neg(x) => -x,
            FuzzReq::Add(a, b) => a + b,
            FuzzReq::Sub(a, b) => a - b,
            FuzzReq::Mul(a, b) => a * b,
            FuzzReq::Div(a, b) => a / b,
            FuzzReq::Rem(a, b) => a % b,
            FuzzReq::MulAdd(a, b, c) => a.mul_add(b, c),
        }
    }
}

impl<RsApF: rustc_apfloat::Float> FuzzReq<RsApF> {
    fn eval_rs_apf(self) -> RsApF {
        match self {
            FuzzReq::Neg(x) => -x,
            FuzzReq::Add(a, b) => (a + b).value,
            FuzzReq::Sub(a, b) => (a - b).value,
            FuzzReq::Mul(a, b) => (a * b).value,
            FuzzReq::Div(a, b) => (a / b).value,
            FuzzReq::Rem(a, b) => (a % b).value,
            FuzzReq::MulAdd(a, b, c) => a.mul_add(b, c).value,
        }
    }
}

fn fuzz_req<HF: HardFloat>(cli_args: &Args, data: &[u8]) -> Option<()> {
    use rustc_apfloat::Float as _;

    // FIXME(eddyb) clean this up.
    let req = {
        let size_in_bytes = {
            assert_eq!(HF::RsApF::BITS % 8, 0);
            HF::RsApF::BITS / 8
        };
        assert_eq!(std::mem::size_of::<HF>(), size_in_bytes);
        assert_eq!(std::mem::size_of::<HF::Bits>(), size_in_bytes);

        let (&op, inputs) = data.split_first()?;
        if inputs.len() % size_in_bytes != 0 {
            return None;
        }

        let mut inputs = inputs.chunks(size_in_bytes).map(HF::bits_from_le_bytes);
        let mut too_few_inputs = false;
        let req = FuzzReq::from_op(op)?.map(|()| {
            inputs.next().unwrap_or_else(|| {
                too_few_inputs = true;
                HF::Bits::default()
            })
        });
        if too_few_inputs || inputs.next().is_some() {
            return None;
        }
        req
    };

    let out_hard = if !cli_args.ignore_hard {
        Some(req.map(HF::from_bits).eval_hard())
    } else {
        None
    };
    let out_rs_apf = req
        .map(|bits| HF::RsApF::from_bits(<u128 as num_traits::NumCast>::from(bits).unwrap()))
        .eval_rs_apf();
    let out_cxx_apf = if cli_args.cxx {
        Some(HF::cxx_apf_eval_fuzz_req(req))
    } else {
        None
    };

    // Allow using `--erase-hard-nans` to hide distinctions only made by hardware.
    let out_hard = if let Some(out_hard) = out_hard {
        Some(if cli_args.erase_hard_nans && out_hard.is_nan() {
            // HACK(eddyb) unsure what else is going on.
            if true {
                assert!(out_rs_apf.is_nan());
                if let Some(out_cxx_apf) = out_cxx_apf {
                    assert!(HF::from_bits(out_cxx_apf).is_nan());
                }
                return Some(());
            }
            HF::nan()
        } else {
            out_hard
        })
    } else {
        None
    };

    let out_rs_apf_bits = <HF::Bits as num_traits::NumCast>::from(out_rs_apf.to_bits()).unwrap();
    if let Some(out_hard) = out_hard {
        assert!(out_hard.to_bits() == out_rs_apf_bits);
    }
    if let Some(out_cxx_apf) = out_cxx_apf {
        // FIXME(eddyb) make this a CLI toggle.
        if out_cxx_apf != out_rs_apf_bits && false {
            eprintln!(
                "{:#08x} != {:#08x}",
                <u64 as num_traits::NumCast>::from(out_cxx_apf).unwrap(),
                <u64 as num_traits::NumCast>::from(out_rs_apf_bits).unwrap()
            );
        }
        assert!(out_cxx_apf == out_rs_apf_bits);
    }

    Some(())
}

fn main() {
    let cli_args = Args::parse();

    // FIXME(eddyb) make `fuzz!` vs `fuzz_nohook!` a CLI toggle.
    fuzz!(|data: &[u8]| {
        data.split_first().and_then(|(bits, data)| match bits {
            32 => fuzz_req::<f32>(&cli_args, data),
            64 => fuzz_req::<f64>(&cli_args, data),
            _ => None,
        });
    });
}
