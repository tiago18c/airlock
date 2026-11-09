//! `PublicCore` reaches no secrets. The compile-time half is the
//! `compile_fail` doc tests on the type; this is the part a doc test cannot say.

use std::marker::PhantomData;
use std::ops::Deref;

use app_core::{Core, PublicCore};

/// `Probe::<T>::DEREFS` is true only when `T: Deref`, because the inherent
/// constant wins over the trait's when its bound holds. The same for the
/// others.
struct Probe<T>(PhantomData<T>);

trait Fallback {
    const DEREFS: bool = false;
    const BECOMES_CORE: bool = false;
}

impl<T> Fallback for Probe<T> {}

impl<T: Deref> Probe<T> {
    const DEREFS: bool = true;
}

impl<T: Deref<Target = PublicCore>> Probe<T> {
    const DEREFS_TO_PUBLIC: bool = true;
}

impl<T: Into<Core>> Probe<T> {
    const BECOMES_CORE: bool = true;
}

#[test]
fn public_core_has_no_secrets() {
    // It must not dereference to `Core`, which would hand back everything.
    const { assert!(!<Probe<PublicCore>>::DEREFS) };
    const { assert!(<Probe<Box<u8>>>::DEREFS, "the probe sees a Deref type") };
    // Nor be turned back into the `Core` it came from.
    const { assert!(!<Probe<PublicCore>>::BECOMES_CORE) };
    const { assert!(<Probe<Core>>::BECOMES_CORE, "the probe sees Into<Core>") };
}

#[test]
fn core_has_what_public_core_has() {
    // The commands both have are written once, on `PublicCore`.
    const { assert!(<Probe<Core>>::DEREFS_TO_PUBLIC) };
}
