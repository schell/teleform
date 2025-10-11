//! Implementations of HasDependencies for primitives.

use crate::v2::{self as tele, HasDependencies};

macro_rules! go {
    ($type: ty) => {
        impl HasDependencies for $type {}
    };
}

go!(String);
go!(u8);
go!(i8);
go!(u16);
go!(i16);
go!(u32);
go!(i32);
go!(u64);
go!(i64);
go!(u128);
go!(i128);
go!(f32);
go!(f64);
go!(bool);

// Rust tuples only implement Default for up to 12 elements, as of now
teleform_derive::impl_has_dependencies_tuples!((A,));
teleform_derive::impl_has_dependencies_tuples!((A, B));
teleform_derive::impl_has_dependencies_tuples!((A, B, C));
teleform_derive::impl_has_dependencies_tuples!((A, B, C, D));
teleform_derive::impl_has_dependencies_tuples!((A, B, C, D, E));
teleform_derive::impl_has_dependencies_tuples!((A, B, C, D, E, F));
teleform_derive::impl_has_dependencies_tuples!((A, B, C, D, E, F, G));
teleform_derive::impl_has_dependencies_tuples!((A, B, C, D, E, F, G, H));
teleform_derive::impl_has_dependencies_tuples!((A, B, C, D, E, F, G, H, I));
teleform_derive::impl_has_dependencies_tuples!((A, B, C, D, E, F, G, H, I, J));
teleform_derive::impl_has_dependencies_tuples!((A, B, C, D, E, F, G, H, I, J, K));
teleform_derive::impl_has_dependencies_tuples!((A, B, C, D, E, F, G, H, I, J, K, L));
teleform_derive::impl_has_dependencies_tuples!((A, B, C, D, E, F, G, H, I, J, K, L, M));

impl<T: HasDependencies> HasDependencies for Vec<T> {
    fn dependencies(&self) -> tele::Dependencies {
        self.iter()
            .fold(tele::Dependencies::default(), |acc, item| {
                acc.merge(item.dependencies())
            })
    }
}

impl<K, V: HasDependencies> HasDependencies for std::collections::HashMap<K, V> {
    fn dependencies(&self) -> tele::Dependencies {
        self.values()
            .fold(tele::Dependencies::default(), |acc, item| {
                acc.merge(item.dependencies())
            })
    }
}

impl<V: HasDependencies> HasDependencies for std::collections::HashSet<V> {
    fn dependencies(&self) -> tele::Dependencies {
        self.iter()
            .fold(tele::Dependencies::default(), |acc, item| {
                acc.merge(item.dependencies())
            })
    }
}

impl<K, V: HasDependencies> HasDependencies for std::collections::BTreeMap<K, V> {
    fn dependencies(&self) -> tele::Dependencies {
        self.values()
            .fold(tele::Dependencies::default(), |acc, item| {
                acc.merge(item.dependencies())
            })
    }
}

impl<V: HasDependencies> HasDependencies for std::collections::BTreeSet<V> {
    fn dependencies(&self) -> tele::Dependencies {
        self.iter()
            .fold(tele::Dependencies::default(), |acc, item| {
                acc.merge(item.dependencies())
            })
    }
}

impl<V: HasDependencies> HasDependencies for Option<V> {
    fn dependencies(&self) -> tele::Dependencies {
        self.iter()
            .fold(tele::Dependencies::default(), |acc, item| {
                acc.merge(item.dependencies())
            })
    }
}
