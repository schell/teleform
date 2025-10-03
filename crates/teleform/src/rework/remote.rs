//! Remote values.
//!
//! Remote values are values that are determined after creating
//! or reading a resource from a provider.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use snafu::OptionExt;

use crate::rework::{DowncastSnafu, MissingResourceSnafu};

use super::{Dependencies, Error, RemoteUnresolvedSnafu, Resource, StoreResource};

#[derive(Clone, Debug)]
enum RemoteInner<Input: Resource, X> {
    Init {
        depends_on: String,
        last_known_value: Option<X>,
    },
    Var {
        map: fn(&Input::Output) -> X,
        var: RemoteVar<Input::Output>,
    },
}

#[derive(Clone, Debug)]
pub struct Remote<Input: Resource, X> {
    inner: RemoteInner<Input, X>,
}

impl<Input: Resource, X: Clone + core::fmt::Debug + PartialEq + 'static> PartialEq
    for Remote<Input, X>
{
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

impl<Input: Resource, X: serde::Serialize + Clone + core::fmt::Debug + 'static> serde::Serialize
    for Remote<Input, X>
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let proxy = RemoteProxy {
            last_known_value: self.get().ok(),
            depends_on: match &self.inner {
                RemoteInner::Init { depends_on, .. } => depends_on.clone(),
                RemoteInner::Var { var, .. } => var.depends_on.clone(),
            },
        };
        proxy.serialize(serializer)
    }
}

impl<'de, Input: Resource, X: serde::Deserialize<'de>> serde::Deserialize<'de>
    for Remote<Input, X>
{
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

impl<T, X: Clone + core::fmt::Debug> Remote<T, X>
where
    T: Resource,
{
    pub(crate) fn new(resource: &StoreResource<T, T::Output>, map: fn(&T::Output) -> X) -> Self {
        log::debug!(
            "creating mapping of a remote resource '{}'",
            resource.remote_var.depends_on
        );
        Self {
            inner: RemoteInner::Var {
                map,
                var: resource.remote_var.clone(),
            },
        }
    }

    pub fn depends_on(&self) -> Dependencies {
        Dependencies {
            inner: vec![match &self.inner {
                RemoteInner::Init { depends_on, .. } => depends_on.clone(),
                RemoteInner::Var { var, .. } => var.depends_on.clone(),
            }],
        }
    }

    pub fn get(&self) -> Result<X, Error> {
        match &self.inner {
            RemoteInner::Init {
                depends_on,
                last_known_value,
            } => {
                log::debug!("remote var returning last known value: {last_known_value:?}");
                Ok(last_known_value.clone().context(RemoteUnresolvedSnafu {
                    ty: core::any::type_name::<X>(),
                    depends_on: depends_on.clone(),
                })?)
            }
            RemoteInner::Var { map, var } => {
                let value = var.get().ok().context(RemoteUnresolvedSnafu {
                    ty: core::any::type_name::<X>(),
                    depends_on: var.depends_on.clone(),
                })?;
                Ok(map(&value))
            }
        }
    }
}

// The state of a remote value.
#[derive(Debug, Default, Clone)]
pub(crate) enum Status<T> {
    #[default]
    None,
    Ok(T),
    Stale(T),
}

impl<T> Status<T> {
    pub fn ok(self) -> Option<T> {
        if let Self::Ok(t) = self {
            Some(t)
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub(crate) struct RemoteVar<T> {
    depends_on: String,
    inner: Arc<Mutex<Status<T>>>,
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
    pub fn get(&self) -> Status<T> {
        self.inner.lock().unwrap().clone()
    }

    pub fn set(&self, status: Status<T>) {
        *self.inner.lock().unwrap() = status;
    }
}

#[derive(Default)]
pub(crate) struct Remotes {
    /// Map of resource name to key + RemoteVar<T>
    vars: HashMap<String, (usize, &'static str, Box<dyn core::any::Any>)>,
}

impl core::fmt::Display for Remotes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, (rez, ty, _)) in self.vars.iter() {
            f.write_fmt(format_args!("name:'{name}' key:{rez} ty:{ty}\n"))?;
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
    pub fn get_var<T: std::any::Any>(
        &mut self,
        id: &str,
    ) -> Result<(RemoteVar<T>, usize, &'static str), Error> {
        log::debug!(
            "requested remote var '{id}' of type {}",
            core::any::type_name::<T>()
        );
        let next_k = self.vars.len();
        let (k, ty, any_var) = self.vars.entry(id.to_owned()).or_insert_with(|| {
            log::debug!("   but one doesn't exist, so we're creating a new entry '{next_k}'");
            (
                next_k,
                std::any::type_name::<T>(),
                Box::new(RemoteVar::<T> {
                    depends_on: id.to_owned(),
                    inner: Default::default(),
                }),
            )
        });
        let var: &RemoteVar<T> = any_var.downcast_ref().context(DowncastSnafu)?;
        Ok((var.clone(), *k, ty))
    }

    /// Returns the name of a resource by key
    pub fn get_name_by_rez(&self, rez: usize) -> Option<String> {
        for (name, (cmp_rez, _, _)) in self.vars.iter() {
            if rez == *cmp_rez {
                return Some(name.clone());
            }
        }
        None
    }

    /// Returns the key of the resource with the given name.
    pub fn get_rez(&self, id: &str) -> Result<usize, Error> {
        Ok(self
            .vars
            .get(id)
            .context(MissingResourceSnafu {
                name: id.to_owned(),
            })?
            .0)
    }
}
