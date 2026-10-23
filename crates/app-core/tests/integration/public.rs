//! `PublicCore` reaches no secrets. The compile-time half is the
//! `compile_fail` doc tests on the type; this is the part a doc test cannot say.

use std::marker::PhantomData;
use std::ops::Deref;

use app_core::PublicCore;

/// `Probe::<T>::DEREFS` is true only when `T: Deref`, because the inherent
/// constant wins over the trait's when its bound holds.
struct Probe<T>(PhantomData<T>);

trait Fallback {
    const DEREFS: bool = false;
}

impl<T> Fallback for Probe<T> {}

impl<T: Deref> Probe<T> {
    const DEREFS: bool = true;
}

#[test]
fn public_core_has_no_secrets() {
    // It must not dereference to `Core`, which would hand back everything.
    const { assert!(!<Probe<PublicCore>>::DEREFS) };
    const { assert!(<Probe<Box<u8>>>::DEREFS, "the probe sees a Deref type") };
}
