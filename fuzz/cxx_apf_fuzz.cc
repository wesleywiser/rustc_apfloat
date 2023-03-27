#include <llvm/ADT/APFloat.h>

using namespace llvm;

#pragma clang diagnostic error "-Wall"
#pragma clang diagnostic error "-Wextra"

struct IEEE32 {
    using Bits = uint32_t;
    static APFloat apf_from_bits(Bits bits) {
        return APFloat(APFloat::IEEEsingle(), APInt(32, bits));
    }
};
struct IEEE64 {
    using Bits = uint64_t;
    static APFloat apf_from_bits(Bits bits) {
        return APFloat(APFloat::IEEEdouble(), APInt(64, bits));
    }
};

template<typename F>
struct FuzzReq {
    enum : uint8_t {
        Neg = 0,

        Add = 1,
        Sub = 2,
        Mul = 3,
        Div = 4,
        Rem = 5,

        MulAdd = 6,
    } op;
    typename F::Bits a_bits, b_bits, c_bits;

    APFloat a() const { return F::apf_from_bits(a_bits); }
    APFloat b() const { return F::apf_from_bits(b_bits); }
    APFloat c() const { return F::apf_from_bits(c_bits); }

    APFloat eval() const {
        switch(op) {
            // FIXME(eddyb) use (unary) `operator-()` with newer LLVM.
            // case Neg: return -a();
            case Neg: return neg(a());
            case Add: return a() + b();
            case Sub: return a() - b();
            case Mul: return a() * b();
            case Div: return a() / b();
            case Rem: {
                APFloat r = a();
                r.mod(b());
                return r;
            }
            case MulAdd: {
                APFloat r = a();
                r.fusedMultiplyAdd(b(), c(), APFloat::rmNearestTiesToEven);
                return r;
            }
        }
    }
    typename F::Bits eval_to_bits() const {
        return eval().bitcastToAPInt().getZExtValue();
    }
};

extern "C" {
    IEEE32::Bits cxx_apf_fuzz_eval_req_ieee32(FuzzReq<IEEE32> req) {
        return req.eval_to_bits();
    }
    IEEE64::Bits cxx_apf_fuzz_eval_req_ieee64(FuzzReq<IEEE64> req) {
        return req.eval_to_bits();
    }
}
