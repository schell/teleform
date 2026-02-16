//! Remote values.
//!
//! Remote values are values that are determined after creating
//! or reading a resource from a provider.

use std::{
    any::Any,
    collections::HashMap,
    ops::Deref,
    sync::{Arc, Mutex},
};

use snafu::OptionExt;

use crate::HasDependencies;

use super::{
    Action, Dependencies, DowncastSnafu, Error, RemoteUnresolvedSnafu, Resource, StoreResource,
};

type VarFn<X> = Arc<dyn Fn(&Arc<dyn Any>) -> Result<X, Error>>;

#[derive(Clone)]
enum RemoteInner<X> {
    Init {
        depends_on: String,
        last_known_value: Option<X>,
    },
    Var {
        depends_on: String,
        map: VarFn<X>,
        // RemoteVar<T::Output>
        var: Arc<dyn Any>,
    },
}

impl<X: std::fmt::Debug> std::fmt::Debug for RemoteInner<X> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Init {
                depends_on,
                last_known_value,
            } => f
                .debug_struct("Init")
                .field("depends_on", depends_on)
                .field("last_known_value", last_known_value)
                .finish(),
            Self::Var {
                depends_on,
                map: _,
                var,
            } => f
                .debug_struct("Var")
                .field("depends_on", depends_on)
                .field("var", var)
                .finish(),
        }
    }
}

#[derive(Clone)]
pub struct Remote<X> {
    inner: RemoteInner<X>,
}

impl<X: Clone + core::fmt::Debug + 'static> std::fmt::Debug for Remote<X> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let depends_on = match &self.inner {
            RemoteInner::Init { depends_on, .. } => depends_on,
            RemoteInner::Var { depends_on, .. } => depends_on,
        };
        f.debug_struct("Remote")
            .field("depends_on", depends_on)
            .field("value", &self.get().ok())
            .finish()
    }
}

impl<X: Clone + core::fmt::Debug + PartialEq + 'static> PartialEq for Remote<X> {
    fn eq(&self, other: &Self) -> bool {
        if let Ok(here) = self.get() {
            if let Ok(there) = other.get() {
                here == there
            } else {
                false
            }
        } else {
            false
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RemoteProxy<T> {
    depends_on: String,
    last_known_value: Option<T>,
}

impl<X: serde::Serialize + Clone + core::fmt::Debug + 'static> serde::Serialize for Remote<X> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let proxy = RemoteProxy {
            last_known_value: self.get().ok(),
            depends_on: match &self.inner {
                RemoteInner::Init { depends_on, .. } => depends_on.clone(),
                RemoteInner::Var { depends_on, .. } => depends_on.clone(),
            },
        };
        proxy.serialize(serializer)
    }
}

impl<'de, X: serde::Deserialize<'de>> serde::Deserialize<'de> for Remote<X> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let RemoteProxy {
            depends_on,
            last_known_value,
        } = RemoteProxy::<X>::deserialize(deserializer)?;

        Ok(Remote {
            inner: RemoteInner::Init {
                depends_on,
                last_known_value,
            },
        })
    }
}

impl<X: Clone + core::fmt::Debug + 'static> Remote<X> {
    pub(crate) fn new<T: Resource>(
        resource: &StoreResource<T, T::Output>,
        map: impl Fn(&T::Output) -> X + 'static,
    ) -> Self {
        log::trace!(
            "creating mapping of a remote resource '{}'",
            resource.remote_var.depends_on
        );
        let depends_on = resource.remote_var.depends_on.clone();
        Self {
            inner: RemoteInner::Var {
                map: Arc::new({
                    let depends_on = depends_on.clone();
                    move |any: &Arc<dyn Any>| {
                        // UNWRAP: safe because this is an invariant
                        let remote_var = any.downcast_ref::<RemoteVar<T::Output>>().unwrap();
                        let t_output = remote_var.get().context(RemoteUnresolvedSnafu {
                            ty: core::any::type_name::<X>(),
                            depends_on: depends_on.clone(),
                        })?;
                        Ok(map(&t_output))
                    }
                }),
                depends_on,
                var: Arc::new(resource.remote_var.clone()),
            },
        }
    }

    pub fn get(&self) -> Result<X, Error> {
        match &self.inner {
            RemoteInner::Init {
                depends_on,
                last_known_value,
            } => {
                log::trace!("remote var returning last known value: {last_known_value:?}");
                Ok(last_known_value.clone().context(RemoteUnresolvedSnafu {
                    ty: core::any::type_name::<X>(),
                    depends_on: depends_on.clone(),
                })?)
            }
            RemoteInner::Var {
                map,
                var,
                depends_on: _,
            } => map(var),
        }
    }

    pub fn map<Y>(&self, f: impl Fn(X) -> Y + 'static) -> Remote<Y> {
        match &self.inner {
            RemoteInner::Init {
                depends_on,
                last_known_value,
            } => Remote {
                inner: RemoteInner::Init {
                    depends_on: depends_on.clone(),
                    last_known_value: last_known_value.clone().map(f),
                },
            },
            RemoteInner::Var {
                depends_on,
                map,
                var,
            } => Remote {
                inner: RemoteInner::Var {
                    depends_on: depends_on.clone(),
                    var: var.clone(),
                    map: Arc::new({
                        let map = map.clone();
                        move |any: &Arc<dyn Any>| {
                            let x = map(any)?;
                            Ok(f(x))
                        }
                    }),
                },
            },
        }
    }
}

impl<X> HasDependencies for Remote<X> {
    fn dependencies(&self) -> Dependencies {
        Dependencies {
            inner: vec![match &self.inner {
                RemoteInner::Init { depends_on, .. } => depends_on.clone(),
                RemoteInner::Var { depends_on, .. } => depends_on.clone(),
            }],
        }
    }
}

#[derive(Debug)]
pub(crate) struct RemoteVar<T> {
    depends_on: String,
    inner: Arc<Mutex<Option<T>>>,
}

impl<T> Clone for RemoteVar<T> {
    fn clone(&self) -> Self {
        Self {
            depends_on: self.depends_on.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<T: Clone> RemoteVar<T> {
    pub fn get(&self) -> Option<T> {
        self.inner.lock().unwrap().clone()
    }

    pub fn set(&self, value: Option<T>) {
        *self.inner.lock().unwrap() = value;
    }
}

pub(crate) struct Var {
    pub(crate) key: usize,
    pub(crate) ty: &'static str,
    pub(crate) action: Action,
    pub(crate) remote: Box<dyn core::any::Any>,
}

#[derive(Default)]
pub(crate) struct Remotes {
    /// Map of resource name to key + RemoteVar<T>
    vars: HashMap<String, Var>,
}

impl core::fmt::Display for Remotes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, var) in self.vars.iter() {
            f.write_fmt(format_args!(
                "name:'{name}' key:{rez} ty:{ty}\n",
                rez = var.key,
                ty = var.ty,
            ))?;
        }
        Ok(())
    }
}

impl Remotes {
    /// Returns a new `RemoteVar<T>` and its resource key.
    ///
    /// ## Errors
    /// Errs if a var by the given name exists but is of a different type than the type
    /// requested.
    pub fn dequeue_var<T: Any>(
        &mut self,
        id: &str,
        action: Action,
    ) -> Result<(RemoteVar<T>, usize, &'static str), Error> {
        log::trace!(
            "requested remote var '{id}' of type {}",
            core::any::type_name::<T>()
        );
        let next_k = self.vars.len();
        let var = self.vars.entry(id.to_owned()).or_insert_with(|| {
            log::trace!("   but one doesn't exist, so we're creating a new entry '{next_k}'");
            Var {
                key: next_k,
                ty: std::any::type_name::<T>(),
                action,
                remote: Box::new(RemoteVar::<T> {
                    depends_on: id.to_owned(),
                    inner: Default::default(),
                }),
            }
        });
        let remote: &RemoteVar<T> = var.remote.downcast_ref().context(DowncastSnafu)?;
        Ok((remote.clone(), var.key, var.ty))
    }

    /// Returns the name of a resource by key
    pub fn get_name_by_rez(&self, rez: usize) -> Option<String> {
        for (name, var) in self.vars.iter() {
            if rez == var.key {
                return Some(name.clone());
            }
        }
        None
    }

    /// Returns the key of the resource with the given name.
    pub fn get(&self, id: &str) -> Option<&Var> {
        self.vars.get(id)
    }

    /// Returns the set of all declared resource IDs.
    pub fn declared_ids(&self) -> std::collections::HashSet<String> {
        self.vars.keys().cloned().collect()
    }

    /// Iterate over all declared resources.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Var)> {
        self.vars.iter()
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum MigratedProxy<T> {
    Remote(RemoteProxy<T>),
    Local(T),
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(try_from = "MigratedProxy<T>")]
pub struct Migrated<T>(pub(crate) T);

impl<T> Deref for Migrated<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> TryFrom<MigratedProxy<T>> for Migrated<T> {
    type Error = &'static str;

    fn try_from(value: MigratedProxy<T>) -> Result<Self, Self::Error> {
        log::trace!("read a migrated {}", std::any::type_name::<T>());
        match value {
            MigratedProxy::Remote(RemoteProxy {
                depends_on: _,
                last_known_value,
            }) => {
                log::trace!("  from a previous remote");
                if let Some(value) = last_known_value {
                    Ok(Migrated(value))
                } else {
                    Err("Missing last known value")
                }
            }
            MigratedProxy::Local(t) => Ok(Migrated(t)),
        }
    }
}

impl<T: serde::Serialize> serde::Serialize for Migrated<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn migrate_ser() {
        let migrated = Migrated(666u32);
        let s = serde_json::to_string_pretty(&migrated).unwrap();
        assert_eq!("666", &s);

        let proxy = MigratedProxy::Remote(RemoteProxy {
            depends_on: "test-bucket".into(),
            last_known_value: Some([109, 121, 98, 117, 99, 107, 101, 116]),
        });
        let s = serde_json::to_string_pretty(&proxy).unwrap();
        println!("{s}");
    }

    #[test]
    fn migrate_de() {
        let s = serde_json::json!({
          "depends_on": "test-bucket",
          "last_known_value": [
            109,
            121,
            98,
            117,
            99,
            107,
            101,
            116
          ]
        });
        let _migrated: Migrated<[u8; 8]> = serde_json::from_value(s).unwrap();
    }
}
