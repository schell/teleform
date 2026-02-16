//! # Teleform
//!
//! Teleform is a library designed to facilitate Infrastructure as Code (IaC)
//! using Rust. It provides a flexible and powerful alternative to tools like
//! Terraform and Pulumi by allowing developers to describe infrastructure
//! changes as a Directed Acyclic Graph (DAG). Unlike other solutions, Teleform
//! does not provide wrappers over platform-specific resources, eschewing them
//! in favor of direct interaction with platform APIs. This removes a layer of
//! indirection and keeps your infrastructure domain specific.
//!
//! ## Key Features
//!
//! - **Resource Management**: Define and manage resources directly through Rust
//!   code, allowing for seamless integration with other libraries.
//! - **Dependency Tracking**: Automatically track dependencies between
//!   resources to ensure correct order of operations.
//! - **Migration Support**: Easily migrate resources and manage changes over
//!   time.
//!
//! ## Usage
//!
//! Teleform is typically used by developers to write custom IaC command line
//! programs executed at a developer workstation.
//!
//! These programs are meant to be fluid, changing as often as the
//! infrastructure, with changes committed and tracked with version control.
//!
//! ### Concepts
//!
//! Teleform operates on the concept of local and remote states of resources:
//!
//! - **Local State**: This is the desired state of the resource as defined in
//!   your Rust code. It represents the initial configuration of a platform
//!   resource.
//! - **Remote State**: This is the state of the resource as it exists on the
//!   platform (e.g., AWS, Digital Ocean). It reflects the configuration
//!   and status of the resource.
//!
//! Teleform uses these states to determine the necessary actions to apply.
//! This involves creating, updating, or deleting resources as needed.
//!
//! An example usage can be found in `crates/teleform/src/test.rs`,
//! demonstrating how to define and manage resources using the library's
//! primitives.
//!
//! ## Target Audience
//!
//! This library is intended for developers, particularly those in solo or small
//! team environments, who are looking for a more general and flexible solution
//! to IaC. It is also suitable for those seeking to migrate away from Terraform.
//!
//! ## Error Handling
//!
//! Teleform exposes a comprehensive error enum [`Error`], which encompasses all
//! possible errors that may occur during operations. Functions that can result
//! in errors return a `Result` type with this [`Error`], ensuring robust error
//! handling throughout the library.

use std::{future::Future, ops::Deref, pin::Pin};

use dagga::{dot::DagLegend, Node, Schedule};
use snafu::prelude::*;
use tokio::io::AsyncWriteExt;

pub use teleform_derive::HasDependencies;

mod has_dependencies_impl;
pub mod remote;
#[cfg(test)]
mod test;
pub mod utils;

use remote::{Migrated, Remote, RemoteVar, Remotes};

/// Marker trait for userland errors.
pub trait UserError: core::fmt::Display + core::fmt::Debug + 'static {}
impl<T: core::fmt::Display + core::fmt::Debug + 'static> UserError for T {}

/// Top-level error enum that encompasses all errors.
#[derive(snafu::Snafu, Debug)]
pub enum Error {
    #[snafu(display("{source}:\n{}",
                source.chain()
                    .map(|e| format!("{e}"))
                    .collect::<Vec<_>>()
                    .join("\n -> ")))]
    Tele { source: anyhow::Error },

    #[snafu(display("Could not read store file '{path:?}': {source}"))]
    StoreFileRead {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Could not delete store file '{path:?}': {source}"))]
    StoreFileDelete {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Could not serialize stored '{name}': {source}"))]
    Serialize {
        name: String,
        source: serde_json::Error,
    },

    #[snafu(display("Could not deserialize stored '{name}': {source}"))]
    Deserialize {
        name: String,
        source: serde_json::Error,
    },

    #[snafu(display("Could not build schedule: {msg}"))]
    Schedule { msg: String },

    #[snafu(display("Could not create file {path:?}: {source}"))]
    CreateFile {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Could not write file {path:?}: {source}"))]
    WriteFile {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Remote value of {ty:?} is unresolved. Depends on {depends_on}"))]
    RemoteUnresolved {
        ty: &'static str,
        depends_on: String,
    },

    #[snafu(display("Could not save the apply graph: {source}"))]
    Dot { source: dagga::dot::DotError },

    #[snafu(display(
        "Could not build apply graph because of a missing resource name for '{missing}'"
    ))]
    MissingName { missing: usize },

    #[snafu(display("Could not find a resource by the name '{name}'"))]
    MissingResource { name: String },

    #[snafu(display("Error during '{name}' creation: {error}"))]
    Create {
        name: String,
        error: Box<dyn UserError>,
    },

    #[snafu(display("Error during '{name}' read and import: {error}"))]
    Import {
        name: String,
        error: Box<dyn UserError>,
    },

    #[snafu(display("Error during '{name}' update: {error}"))]
    Update {
        name: String,
        error: Box<dyn UserError>,
    },

    #[snafu(display("Error during '{name}' destruction: {error}"))]
    Destroy {
        name: String,
        error: Box<dyn UserError>,
    },

    #[snafu(display("Error during execution of a manual step '{name}': {error}"))]
    Manual {
        name: String,
        error: Box<dyn UserError>,
    },

    #[snafu(display("Missing previous remote value '{name}'"))]
    Load { name: String },

    #[snafu(display(
        "Loading '{id}' would clobber an existing value in the store file, \
        and these values are not the same"
    ))]
    Clobber { id: String },

    #[snafu(display("Could not downcast"))]
    Downcast,

    #[snafu(display("Missing store file for '{id}'"))]
    MissingStoreFile { id: String },

    #[snafu(display("Could not scan store directory '{path:?}': {source}"))]
    ScanStoreDir {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
}

impl From<anyhow::Error> for Error {
    fn from(source: anyhow::Error) -> Self {
        Error::Tele { source }
    }
}

impl From<dagga::dot::DotError> for Error {
    fn from(source: dagga::dot::DotError) -> Self {
        Self::Dot { source }
    }
}

type Result<T, E = Error> = core::result::Result<T, E>;

/// IaC resources.
///
/// Represents a resource created on a platform (ie AWS, Digital Ocean, etc).
#[allow(unreachable_code)]
pub trait Resource:
    core::fmt::Debug
    + Clone
    + PartialEq
    + HasDependencies
    + serde::Serialize
    + serde::de::DeserializeOwned
    + 'static
{
    /// Type of the platform/resource provider.
    ///
    /// For example `aws_config::SdkConfig` in the case of amazon web services.
    type Provider;

    /// Errors that may occur interacting with the provider.
    type Error: UserError;

    /// The remote type of this resource, which we can used to fill in
    /// [`Remote`] values in other resources.
    type Output: core::fmt::Debug
        + Clone
        + PartialEq
        + serde::Serialize
        + serde::de::DeserializeOwned
        + 'static;

    /// Creates a new resource on the platform.
    ///
    /// This method should be implemented to define how a resource is created
    /// using the provider's API. It returns a future that resolves to the
    /// resource's output type or an error.
    ///
    /// ## Note
    /// This method is explicitly `unimplemented!` for developer convenience.
    /// It allows you to define only the methods you need. However, take care when
    /// using this in contexts like long-running daemons, as calling an unimplemented
    /// method will cause a panic.
    fn create(
        &self,
        _provider: &Self::Provider,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> {
        unimplemented!(
            "Resource::create is unimplemented for {}",
            std::any::type_name::<Self>()
        ) as Box<dyn Future<Output = Result<_, _>> + Unpin>
    }

    /// Reads the current state of the resource from the platform.
    ///
    /// This method should be implemented to define how to fetch the current
    /// state of a resource using the provider's API. It returns a future that
    /// resolves to the resource's output type or an error.
    ///
    /// ## Note
    /// This method is explicitly `unimplemented!` for developer convenience.
    /// It allows you to define only the methods you need. However, take care when
    /// using this in contexts like long-running daemons, as calling an unimplemented
    /// method will cause a panic.
    fn read(
        &self,
        _provider: &Self::Provider,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> {
        unimplemented!(
            "Resource::read is unimplemented for {}",
            std::any::type_name::<Self>()
        ) as Box<dyn Future<Output = Result<_, _>> + Unpin>
    }

    /// Updates an existing resource on the platform.
    ///
    /// This method should be implemented to define how a resource is updated
    /// using the provider's API. It takes the previous local and remote states
    /// of the resource and returns a future that resolves to the updated
    /// resource's output type or an error.
    ///
    /// ## Note
    /// This method is explicitly `unimplemented!` for developer convenience.
    /// It allows you to define only the methods you need. However, take care when
    /// using this in contexts like long-running daemons, as calling an unimplemented
    /// method will cause a panic.
    fn update(
        &self,
        _provider: &Self::Provider,
        _previous_local: &Self,
        _previous_remote: &Self::Output,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> {
        unimplemented!(
            "Resource::update is unimplemented for {}",
            std::any::type_name::<Self>()
        ) as Box<dyn Future<Output = Result<_, _>> + Unpin>
    }

    /// Deletes a resource from the platform.
    ///
    /// This method should be implemented to define how a resource is deleted
    /// using the provider's API. It takes the previous remote state of the
    /// resource and returns a future that resolves to a unit type or an error.
    ///
    /// ## Note
    /// This method is explicitly `unimplemented!` for developer convenience.
    /// It allows you to define only the methods you need. However, take care when
    /// using this in contexts like long-running daemons, as calling an unimplemented
    /// method will cause a panic.
    fn delete(
        &self,
        _provider: &Self::Provider,
        _previous_remote: &Self::Output,
    ) -> impl Future<Output = Result<(), Self::Error>> {
        unimplemented!(
            "Resource::delete is unimplemented for {}",
            std::any::type_name::<Self>()
        ) as Box<dyn Future<Output = Result<_, _>> + Unpin>
    }
}

#[derive(Clone, Default, Debug)]
pub struct Dependencies {
    /// Specifies a dependency on a `Resource`.
    inner: Vec<String>,
}

impl IntoIterator for Dependencies {
    type Item = String;

    type IntoIter = <Vec<String> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}

impl core::fmt::Display for Dependencies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            &self
                .inner
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

impl Dependencies {
    pub fn merge(self, other: Self) -> Self {
        Dependencies {
            inner: [self.inner, other.inner].concat(),
        }
    }
}

/// Tracks dependencies between resources.
///
/// This trait can be derived, and has a default implementation that
/// reports zero dependencies.
pub trait HasDependencies {
    fn dependencies(&self) -> Dependencies {
        Dependencies::default()
    }
}

/// `Create`, `Load` and `Update` result in a resource being added to the graph.
///
/// `Destroy` moves the resource out of the graph.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Action {
    Load,
    Create,
    Read,
    Update,
    Destroy,
}

impl core::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Action::Load => "load",
            Action::Create => "create",
            Action::Read => "read",
            Action::Update => "update",
            Action::Destroy => "destroy",
        })
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct InertStoreResource {
    name: String,
    local: serde_json::Value,
    remote: serde_json::Value,
    /// The Rust type name of the resource (via `std::any::type_name::<T>()`).
    /// Used for orphan detection and auto-deletion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    type_name: Option<String>,
    /// The resource IDs this resource depends on.
    /// Used for ordering orphan deletions correctly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dependencies: Option<Vec<String>>,
}

impl InertStoreResource {
    async fn save(
        &self,
        resource_id: &str,
        store_path: impl AsRef<std::path::Path>,
    ) -> Result<(), Error> {
        let path = store_file_path(resource_id, &store_path);
        log::info!("storing {resource_id} to {path:?}");

        let contents = serde_json::to_string_pretty(self).context(SerializeSnafu {
            name: format!("storing {}", resource_id),
        })?;

        // Ensure the parent directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(&parent)
                .await
                .context(CreateFileSnafu { path: parent })?;
        }

        let mut file = tokio::fs::File::create(&path)
            .await
            .context(CreateFileSnafu { path: path.clone() })?;
        file.write_all(contents.as_bytes())
            .await
            .context(WriteFileSnafu { path: path.clone() })?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct StoreResource<L, R> {
    /// Name of the resource from the user's perspective
    name: String,
    /// Local definition in _code_
    local_definition: L,
    action: Action,
    remote_var: RemoteVar<R>,
}

impl<L, R> Deref for StoreResource<L, R> {
    type Target = L;

    fn deref(&self) -> &Self::Target {
        &self.local_definition
    }
}

impl<L, R> AsRef<L> for StoreResource<L, R> {
    fn as_ref(&self) -> &L {
        &self.local_definition
    }
}

impl<L, R> TryFrom<StoreResource<L, R>> for InertStoreResource
where
    L: serde::Serialize + for<'a> serde::Deserialize<'a>,
    R: Clone + serde::Serialize + for<'a> serde::Deserialize<'a>,
{
    type Error = Error;

    fn try_from(value: StoreResource<L, R>) -> std::result::Result<Self, Self::Error> {
        let local = serde_json::to_value(value.local_definition).context(SerializeSnafu {
            name: value.name.clone(),
        })?;
        let output = value.remote_var.get().context(LoadSnafu {
            name: value.name.clone(),
        })?;
        let remote = serde_json::to_value(output).context(SerializeSnafu {
            name: value.name.clone(),
        })?;
        Ok(Self {
            name: value.name,
            local,
            remote,
            type_name: None,
            dependencies: None,
        })
    }
}

impl<T> StoreResource<T, T::Output>
where
    T: Resource,
    T::Output: Clone,
{
    /// Map a remote value to use in local definitions.
    pub fn remote<X: Clone + core::fmt::Debug + 'static>(
        &self,
        f: impl Fn(&T::Output) -> X + 'static,
    ) -> Remote<X> {
        Remote::new(self, f)
    }

    /// Return the action that would be applied to this resource.
    ///
    /// This is useful if you need to trigger invalidations or anything else based on
    /// whether a resource is created, updated, deleted, etc.
    pub fn action(&self) -> Action {
        self.action
    }

    pub fn depends_on<X, Y>(&self, store: &mut Store<T::Provider>, resource: &StoreResource<X, Y>) {
        let this_var = store.remotes.get(&self.name).unwrap();
        let that_var = store.remotes.get(&resource.name).unwrap();
        for node in store.graph.take_nodes() {
            store.graph.add_node(
                if node
                    .get_results()
                    .copied()
                    .collect::<Vec<_>>()
                    .contains(&this_var.key)
                {
                    node.with_read(that_var.key)
                } else {
                    node
                },
            );
        }
    }
}

/// The path to an individual resource store file.
fn store_file_path(name: &str, store_path: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    store_path.as_ref().join(format!("{name}.json"))
}

/// Extract `depends_on` resource IDs from a serialized local definition.
///
/// Walks the JSON tree looking for `{"depends_on": "..."}` patterns,
/// which is how [`Remote`] serializes via `RemoteProxy`.
fn extract_depends_on_from_json(value: &serde_json::Value) -> Vec<String> {
    let mut deps = Vec::new();
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(dep)) = map.get("depends_on") {
                deps.push(dep.clone());
            }
            for v in map.values() {
                deps.extend(extract_depends_on_from_json(v));
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                deps.extend(extract_depends_on_from_json(v));
            }
        }
        _ => {}
    }
    deps
}

type StoreNodeRunFn<Provider> = Box<
    dyn FnOnce(
        // Resource platform provider
        &'_ Provider,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + '_>>,
>;

struct RunAction<'a, Provider, T: Resource<Provider = Provider>> {
    provider: &'a Provider,
    store_path: std::path::PathBuf,
    /// Name of the resource being acted on, not the node name.
    resource_id: String,
    action: Action,
    local_definition_code: T,
    local_definition_store: Option<T>,
    remote_var: RemoteVar<T::Output>,
}

impl<Provider, T: Resource<Provider = Provider>> RunAction<'_, Provider, T> {
    async fn run(self) -> Result<()>
    where
        T: Resource,
    {
        let Self {
            provider,
            store_path,
            resource_id,
            action,
            local_definition_code,
            local_definition_store,
            remote_var,
        } = self;
        log::info!("{action} '{resource_id}':");

        async fn save<T: Resource>(
            resource_id: &str,
            local_definition_code: T,
            remote_var: &RemoteVar<T::Output>,
            store_path: impl AsRef<std::path::Path>,
        ) -> Result<(), Error> {
            let deps: Vec<String> = local_definition_code.dependencies().into_iter().collect();
            let inert_resource = InertStoreResource {
                name: resource_id.to_owned(),
                local: serde_json::to_value(&local_definition_code).context(SerializeSnafu {
                    name: format!("store {resource_id}"),
                })?,
                remote: serde_json::to_value(
                    remote_var.get().context(LoadSnafu { name: resource_id })?,
                )
                .context(SerializeSnafu {
                    name: format!("store {resource_id} remote"),
                })?,
                type_name: Some(std::any::type_name::<T>().to_owned()),
                dependencies: if deps.is_empty() { None } else { Some(deps) },
            };
            inert_resource.save(resource_id, store_path).await?;
            Ok(())
        }

        match action {
            Action::Load => {
                save(&resource_id, local_definition_code, &remote_var, store_path).await?;
            }
            Action::Create => {
                let value = local_definition_code
                    .create(provider)
                    .await
                    .map_err(|error| Error::Create {
                        name: resource_id.to_owned(),
                        error: Box::new(error),
                    })?;
                remote_var.set(Some(value));
                save(&resource_id, local_definition_code, &remote_var, store_path).await?;
            }
            Action::Read => {
                let value = local_definition_code
                    .read(provider)
                    .await
                    .map_err(|error| Error::Create {
                        name: resource_id.to_owned(),
                        error: Box::new(error),
                    })?;
                remote_var.set(Some(value));
                save(&resource_id, local_definition_code, &remote_var, store_path).await?;
            }
            Action::Update => {
                let previous_local = local_definition_store.unwrap();
                let previous_remote = remote_var.get().context(LoadSnafu {
                    name: resource_id.clone(),
                })?;
                if previous_local == local_definition_code {
                    log::warn!(
                        "Skipping '{resource_id}' update as the local value has not changed.\n\
                        If you require an update, consider adding a sentinel value."
                    );
                } else {
                    let cmp =
                        pretty_assertions::Comparison::new(&previous_local, &local_definition_code);
                    let change_string = format!("{cmp}")
                        .lines()
                        .map(|line| format!("  {line}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    log::info!("updating '{resource_id}':\n{change_string}");
                    let output = local_definition_code
                        .update(provider, &previous_local, &previous_remote)
                        .await
                        .map_err(|error| Error::Update {
                            name: resource_id.clone(),
                            error: Box::new(error),
                        })?;
                    remote_var.set(Some(output));
                    save(&resource_id, local_definition_code, &remote_var, store_path).await?;
                }
            }
            Action::Destroy => {
                log::debug!("running destroy action on {resource_id}");
                // In the destroy case there is no code-local definition, but there is always
                // a store definition, so we pass the store definition as the code definition.
                // This is better IMO than having both code-local and store be optional.
                let local_definition = local_definition_code.clone();
                let previous_remote = remote_var.get().context(LoadSnafu {
                    name: resource_id.clone(),
                })?;
                local_definition
                    .delete(provider, &previous_remote)
                    .await
                    .map_err(|error| Error::Destroy {
                        name: resource_id.to_owned(),
                        error: Box::new(error),
                    })?;

                log::info!("  {resource_id} is destroyed");
                let path = store_file_path(&resource_id, &store_path);
                log::info!("  removing {resource_id} store file {path:?}");
                tokio::fs::remove_file(&path)
                    .await
                    .context(StoreFileDeleteSnafu { path })?;
                remote_var.set(None);
            }
        }

        log::info!("  success!");
        Ok(())
    }
}

pub struct DestroyResource<T: Resource> {
    local: T,
    remote: T::Output,
}

impl<T: Resource> Deref for DestroyResource<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.local
    }
}

impl<T: Resource> DestroyResource<T> {
    /// Map a remote value of a resource scheduled to be destroyed into a
    /// permanent field of another resource.
    pub fn migrate<X: Clone + core::fmt::Debug + 'static>(
        &self,
        f: fn(&T::Output) -> X,
    ) -> Migrated<X> {
        Migrated(f(&self.remote))
    }
}

struct StoreNode<Provider> {
    name: String,
    _remote_ty: &'static str,
    run: StoreNodeRunFn<Provider>,
}

struct PreviouslyStored<T: Resource> {
    action: Action,
    resource: Option<(T, T::Output)>,
}

/// A type-erased delete function for a specific resource type.
///
/// Constructed automatically when a resource type is first used (via
/// [`Store::resource`], [`Store::import`], [`Store::load`], or
/// [`Store::destroy`]), or manually via [`Store::register`]. Produces a
/// [`StoreNodeRunFn`] that reads the store file, deserializes it into the
/// concrete type, calls `T::delete()`, and removes the file.
struct ResourceDeleter<Provider> {
    make_run_fn: Box<
        dyn Fn(
            std::path::PathBuf, // store_path
            String,             // resource_id
        ) -> StoreNodeRunFn<Provider>,
    >,
}

/// A single planned action for a resource.
#[derive(Clone, Debug)]
pub struct PlannedAction {
    /// The resource ID.
    pub id: String,
    /// The action to be taken.
    pub action: Action,
    /// The Rust type name, if known.
    pub type_name: Option<String>,
    /// Whether this is an auto-detected orphan.
    pub is_orphan: bool,
}

/// A plan of actions produced by [`Store::plan`].
///
/// Inspect the plan before passing it to [`Store::apply`] to execute.
pub struct Plan<Provider> {
    /// The planned actions, in no particular order.
    pub actions: Vec<PlannedAction>,
    /// Resources that appear orphaned but could not be auto-deleted
    /// (unregistered type or missing `type_name` in store file).
    pub warnings: Vec<String>,
    /// Internal: the built schedule.
    schedule: Schedule<Node<StoreNode<Provider>, usize>>,
}

impl<Provider> core::fmt::Display for Plan<Provider> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.actions.is_empty() {
            f.write_str("No changes.\n")?;
            return Ok(());
        }
        for action in &self.actions {
            let orphan_marker = if action.is_orphan { " (orphan)" } else { "" };
            let ty = action.type_name.as_deref().unwrap_or("unknown");
            writeln!(
                f,
                "  {} '{}' [{}]{}",
                action.action, action.id, ty, orphan_marker
            )?;
        }
        for warning in &self.warnings {
            writeln!(f, "  WARNING: {warning}")?;
        }
        Ok(())
    }
}

pub struct Store<T> {
    path: std::path::PathBuf,
    provider: T,
    remotes: Remotes,
    graph: dagga::Dag<StoreNode<T>, usize>,
    deleters: std::collections::HashMap<String, ResourceDeleter<T>>,
}

impl<P: 'static> Store<P> {
    fn read_from_store<T: Resource<Provider = P>>(
        path: impl AsRef<std::path::Path>,
        id: &str,
    ) -> Result<(T, T::Output)> {
        let path = store_file_path(id, path.as_ref());
        snafu::ensure!(path.exists(), MissingStoreFileSnafu { id: id.to_owned() });

        log::debug!("{path:?} exists, reading '{id}' from it");
        let contents = std::fs::read_to_string(&path).context(StoreFileReadSnafu {
            path: path.to_path_buf(),
        })?;
        log::trace!(
            "contents:\n{}",
            contents
                .lines()
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let inert_store_rez: InertStoreResource =
            serde_json::from_str(&contents).context(DeserializeSnafu {
                name: id.to_owned(),
            })?;
        log::trace!("read inert store resource");
        log::trace!(
            "reading local contents: {}",
            serde_json::to_string_pretty(&inert_store_rez.local)
                .unwrap()
                .lines()
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        log::trace!("as {}", std::any::type_name::<T>());
        let stored_definition: T =
            serde_json::from_value(inert_store_rez.local).context(DeserializeSnafu {
                name: id.to_owned(),
            })?;

        log::trace!("  reading remote output JSON value");
        let remote_value: T::Output =
            serde_json::from_value(inert_store_rez.remote).context(DeserializeSnafu {
                name: format!("remote {id}"),
            })?;
        Ok((stored_definition, remote_value))
    }

    pub fn new(path: impl AsRef<std::path::Path>, provider: P) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            graph: dagga::Dag::default(),
            remotes: Default::default(),
            provider,
            deleters: Default::default(),
        }
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// Ensure a resource type is registered for orphan auto-deletion.
    ///
    /// This is called automatically by [`Store::resource`],
    /// [`Store::import`], [`Store::load`], and [`Store::destroy`], so
    /// manual calls are only needed for types that are **not** declared in
    /// the current run but may still have leftover store files from a
    /// previous apply.
    fn ensure_registered<T>(&mut self)
    where
        T: Resource<Provider = P>,
    {
        let type_name = std::any::type_name::<T>();
        if self.deleters.contains_key(type_name) {
            return;
        }
        self.deleters.insert(
            type_name.to_owned(),
            ResourceDeleter {
                make_run_fn: Box::new(|store_path, resource_id| {
                    Box::new(move |provider: &P| {
                        Box::pin(async move {
                            let (local, remote): (T, T::Output) =
                                Self::read_from_store(&store_path, &resource_id)?;
                            log::info!("destroy '{resource_id}' (orphan auto-delete):");
                            local.delete(provider, &remote).await.map_err(|error| {
                                Error::Destroy {
                                    name: resource_id.clone(),
                                    error: Box::new(error),
                                }
                            })?;
                            let path = store_file_path(&resource_id, &store_path);
                            log::info!("  removing {resource_id} store file {path:?}");
                            tokio::fs::remove_file(&path)
                                .await
                                .context(StoreFileDeleteSnafu { path })?;
                            log::info!("  {resource_id} destroyed");
                            Ok(())
                        }) as Pin<Box<dyn Future<Output = Result<()>> + '_>>
                    })
                }),
            },
        );
    }

    /// Register a resource type for automatic orphan detection and deletion.
    ///
    /// When [`Store::plan`] discovers a store file whose `type_name` matches
    /// this type but no corresponding [`Store::resource`] or
    /// [`Store::destroy`] call was made, it will schedule the resource for
    /// automatic deletion.
    ///
    /// ## Note
    ///
    /// Resource types are now **automatically registered** whenever they are
    /// used via [`Store::resource`], [`Store::import`], [`Store::load`], or
    /// [`Store::destroy`]. You only need to call this method for resource
    /// types that are **not** declared in the current run but may still have
    /// orphaned store files from a previous apply.
    pub fn register<T>(&mut self) -> &mut Self
    where
        T: Resource<Provider = P>,
    {
        self.ensure_registered::<T>();
        self
    }

    fn read_file<T>(&self, id: &str) -> Result<(T, T::Output), Error>
    where
        T: Resource<Provider = P>,
    {
        Self::read_from_store(&self.path, id)
    }

    /// Adds a barrier after which all resources will be run after those defined before.
    pub fn barrier(&mut self) {
        self.graph.add_barrier();
    }

    fn define_resource<T>(
        &mut self,
        id: impl AsRef<str>,
        local_definition: T,
        action: Action,
        stored_definition: Option<T>,
        output: Option<T::Output>,
    ) -> Result<StoreResource<T, T::Output>, Error>
    where
        T: Resource<Provider = P>,
    {
        self.ensure_registered::<T>();
        let id = id.as_ref();
        let (remote_var, rez, _ty) = self.remotes.dequeue_var::<T::Output>(id, action)?;
        remote_var.set(output);

        let remote_var = remote_var.clone();
        let local_definition_code = local_definition.clone();
        let local_definition_store = stored_definition.clone();
        let store_path = self.path.clone();
        let run: StoreNodeRunFn<T::Provider> = Box::new({
            let resource_id = id.to_owned();
            let remote_var = remote_var.clone();
            let local_definition_code = local_definition_code.clone();
            let local_definition_store = local_definition_store.clone();
            move |provider: &T::Provider| {
                Box::pin(
                    RunAction {
                        provider,
                        store_path,
                        resource_id,
                        action,
                        local_definition_code,
                        local_definition_store,
                        remote_var,
                    }
                    .run(),
                )
            }
        });
        let ty = std::any::type_name::<T>();

        {
            // Add the main action node
            log::debug!("adding main node {action} {id}");
            let node_name = format!("{action} {id}");
            let dag_node = dagga::Node::new(StoreNode {
                name: node_name.clone(),
                _remote_ty: ty,
                run,
            })
            .with_name(node_name)
            .with_reads({
                // read the resource keys out of "remotes" as dependencies
                let mut reads = vec![];
                for dep in local_definition.dependencies() {
                    let var = self
                        .remotes
                        .get(&dep)
                        .context(MissingResourceSnafu { name: dep })?;
                    reads.push(var.key);
                }
                reads
            });
            let dag_node = match action {
                Action::Create | Action::Read | Action::Load | Action::Update => {
                    log::debug!("  with result {rez}");
                    dag_node.with_result(rez)
                }
                Action::Destroy => {
                    log::debug!("  with move {rez}");
                    dag_node.with_move(rez)
                }
            };
            self.graph.add_node(dag_node);
        }

        Ok(StoreResource {
            name: id.to_owned(),
            local_definition,
            action,
            remote_var,
        })
    }

    /// Read the stored previous definition and determine the action.
    fn determine_action_from_previously_stored<T>(
        &self,
        local_definition: &T,
        id: &str,
    ) -> Result<PreviouslyStored<T>, Error>
    where
        T: Resource<Provider = P>,
    {
        match self.read_file(id) {
            Ok((stored_definition, output)) => {
                // This has already been created and stored, so this is either a simple load,
                // or an update.
                log::debug!("  {output:?}");
                let action = if *local_definition != stored_definition {
                    log::debug!("  local resource has changed, so this remote is now stale");
                    Action::Update
                } else {
                    // Check if any upstream dependencies are "stale" (updated or deleted),
                    // which would cause this resource to possibly require an update.
                    let mut may_need_update = false;
                    for dep in local_definition.dependencies() {
                        let var = self.remotes.get(&dep).context(LoadSnafu { name: dep })?;
                        if var.action != Action::Load {
                            may_need_update = true;
                            break;
                        }
                    }
                    if may_need_update {
                        Action::Update
                    } else {
                        Action::Load
                    }
                };

                Ok(PreviouslyStored {
                    action,
                    resource: Some((stored_definition, output)),
                })
            }
            Err(Error::MissingStoreFile { id }) => {
                log::debug!("store file '{id}' does not exist, creating a new resource",);
                Ok(PreviouslyStored {
                    action: Action::Create,
                    resource: None,
                })
            }
            Err(e) => {
                log::error!("could not define resource '{id}': {e}");
                Err(e)
            }
        }
    }

    /// Defines a resource.
    ///
    /// Produces two graph nodes:
    /// 1. Depending on the result of compairing `local_definition` to the one on file
    ///    (if it exists), either:
    ///    - creates the resource on the platform
    ///    - updates the resource on the platform
    ///    - loads the resource from a file
    /// 2. Stores the resource to a file
    ///
    /// To import an existing resource from a platform, use [`Store::import`].
    pub fn resource<T>(
        &mut self,
        id: impl AsRef<str>,
        local_definition: T,
    ) -> Result<StoreResource<T, T::Output>, Error>
    where
        T: Resource<Provider = P>,
    {
        let id = id.as_ref();
        let PreviouslyStored { action, resource } =
            self.determine_action_from_previously_stored(&local_definition, id)?;
        let (local, remote) = resource
            .map(|(local, remote)| (Some(local), Some(remote)))
            .unwrap_or_default();
        self.define_resource(id, local_definition, action, local, remote)
    }

    /// Defines a pre-existing resource, importing it from the platform.
    ///
    /// Produces two graph nodes:
    /// 1. Import the resource from the platform, resulting in the resource
    /// 2. Store the value to a file
    ///
    /// This only needs to be used once in your infrastructure command.
    /// After the resource is imported and stored to a file it is recommended
    /// you make a code change to use [`Store::resource`].
    pub fn import<T>(
        &mut self,
        id: impl AsRef<str>,
        local_definition: T,
    ) -> Result<StoreResource<T, T::Output>, Error>
    where
        T: Resource<Provider = P>,
    {
        self.define_resource(id, local_definition, Action::Read, None, None)
    }

    /// Defines a pre-existing resource, directly writing it to file, without
    /// querying the platform.
    ///
    /// Produces two graph nodes:
    /// 1. Load the value (noop)
    /// 2. Store the value
    ///
    /// ## Errors
    /// Errs if `force_overwrite` is `false` _and_ a stored resource already
    /// exists. This is done to prevent accidental clobbering.
    pub fn load<T>(
        &mut self,
        id: impl AsRef<str>,
        local_definition: T,
        remote_definition: T::Output,
        force_overwrite: bool,
    ) -> Result<StoreResource<T, T::Output>, Error>
    where
        T: Resource<Provider = P>,
    {
        let id = id.as_ref();
        if let Ok((stored_definition, output)) = self.read_file(id) {
            if local_definition == stored_definition && remote_definition == output {
                if force_overwrite {
                    log::warn!("loading '{id}' is clobbering an existing value, but `force_overwrite` is `true`");
                } else {
                    let err = ClobberSnafu { id: id.to_owned() }.build();
                    log::error!("{err}");
                    return Err(err);
                }
            }
        }
        self.define_resource(
            id,
            local_definition,
            Action::Load,
            None,
            Some(remote_definition),
        )
    }

    /// Destroys a resource.
    pub fn destroy<T>(&mut self, id: impl AsRef<str>) -> Result<DestroyResource<T>, Error>
    where
        T: Resource<Provider = P>,
    {
        self.ensure_registered::<T>();
        let id = id.as_ref();
        let (local, remote) = self.read_file::<T>(id)?;
        let (remote_var, rez, _ty) = self.remotes.dequeue_var::<T::Output>(id, Action::Destroy)?;
        remote_var.set(Some(remote.clone()));
        {
            // Destruction requires a load to introduce the resource (for the DAG)
            log::debug!("adding node {} {id}", Action::Load);
            let node_name = format!("load {id}");
            let load_node = dagga::Node::new(StoreNode {
                name: node_name.clone(),
                _remote_ty: std::any::type_name::<T>(),
                run: Box::new({
                    let resource_id = id.to_owned();
                    let store_path = self.path.clone();
                    let local = local.clone();
                    let remote_var = remote_var.clone();
                    move |provider| {
                        Box::pin(
                            RunAction {
                                provider,
                                store_path,
                                resource_id,
                                action: Action::Load,
                                local_definition_code: local,
                                remote_var,
                                local_definition_store: None,
                            }
                            .run(),
                        )
                    }
                }),
            })
            .with_name(node_name)
            .with_reads({
                let mut reads = vec![];
                for dep in local.dependencies() {
                    reads.push(
                        self.remotes
                            .get(&dep)
                            .context(MissingResourceSnafu {
                                name: id.to_owned(),
                            })?
                            .key,
                    );
                }
                reads
            })
            .with_result(rez);
            self.graph.add_node(load_node);
        }
        {
            log::debug!("adding node {} {id}", Action::Destroy);
            let node_name = format!("destroy {id}");
            // Add the destroy node
            let destroy_node = StoreNode {
                name: node_name.clone(),
                _remote_ty: std::any::type_name::<T>(),
                run: Box::new({
                    let resource_id = id.to_owned();
                    let local = local.clone();
                    let store_path = self.path.clone();
                    let remote_var = remote_var.clone();
                    move |provider| {
                        Box::pin(
                            RunAction {
                                provider,
                                store_path,
                                resource_id,
                                action: Action::Destroy,
                                local_definition_code: local,
                                local_definition_store: None,
                                remote_var,
                            }
                            .run(),
                        )
                    }
                }),
            };

            self.graph.add_node(
                dagga::Node::new(destroy_node)
                    .with_name(node_name)
                    .with_move(rez),
            );
        }

        Ok(DestroyResource { local, remote })
    }

    fn get_graph_legend(&self) -> Result<DagLegend<usize>> {
        let mut missing_resource_creation = None;
        let legend = self.graph.legend()?.with_resources_named(|rez| {
            let maybe_name = self.remotes.get_name_by_rez(*rez);
            if maybe_name.is_none() {
                missing_resource_creation = Some(*rez);
            }
            maybe_name
        });
        if let Some(missing) = missing_resource_creation {
            log::error!(
                "Missing resource {missing}, current resources:\n{}",
                self.remotes
            );
            return MissingNameSnafu { missing }.fail();
        }
        Ok(legend)
    }

    pub fn get_schedule_string(&self) -> Result<String, Error> {
        let mut dag: dagga::Dag<(), usize> = dagga::Dag::default();
        for node in self.graph.nodes() {
            let store_node = node.inner();
            let print_node = dagga::Node::new(())
                .with_name(store_node.name.clone())
                .with_reads(node.get_reads().copied())
                .with_results(node.get_results().copied())
                .with_moves(node.get_moves().copied())
                .with_barrier(node.get_barrier());
            dag.add_node(print_node);
        }
        struct Proxy {
            inner: Schedule<Node<(), usize>>,
        }

        impl core::fmt::Display for Proxy {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                if self.inner.batches.is_empty() {
                    f.write_str("--- No changes.\n")?;
                    f.write_str("--- 🌈🦄\n")?;
                }
                for (i, batch) in self.inner.batches.iter().enumerate() {
                    let i = i + 1;
                    f.write_str("--- step ")?;
                    f.write_fmt(format_args!("{i}\n"))?;
                    for node in batch.iter() {
                        f.write_str("  ")?;
                        f.write_str(node.name())?;
                        f.write_str("\n")?;
                    }
                    f.write_str("---\n")?;
                }
                Ok(())
            }
        }

        let proxy = Proxy {
            inner: dag.build_schedule().unwrap(),
        };
        Ok(proxy.to_string())
    }

    pub fn save_apply_graph(&self, path: impl AsRef<std::path::Path>) -> Result<(), Error> {
        if self.graph.is_empty() {
            log::warn!("Resource DAG is empty, writing an empty dot file");
        }
        let legend = self.get_graph_legend()?;
        dagga::dot::save_as_dot(&legend, path).context(DotSnafu)?;

        Ok(())
    }

    /// Scan the store directory and build an execution plan.
    ///
    /// Compares declared resources (from [`Store::resource`],
    /// [`Store::destroy`], etc.) against store files on disk. Resources
    /// found on disk but not declared are flagged as orphans.
    ///
    /// Orphans whose types are registered via [`Store::register`] are
    /// automatically scheduled for deletion. Unregistered orphans produce
    /// warnings.
    pub fn plan(&mut self) -> Result<Plan<P>> {
        let mut actions = Vec::new();
        let mut warnings = Vec::new();

        // Collect declared resource IDs
        let declared_ids = self.remotes.declared_ids();

        // Collect actions for declared resources
        for (id, var) in self.remotes.iter() {
            actions.push(PlannedAction {
                id: id.clone(),
                action: var.action,
                type_name: Some(var.ty.to_owned()),
                is_orphan: false,
            });
        }

        // Scan the store directory for .json files to detect orphans
        let store_dir = &self.path;
        if store_dir.exists() {
            let entries = std::fs::read_dir(store_dir).context(ScanStoreDirSnafu {
                path: store_dir.clone(),
            })?;

            for entry in entries {
                let entry = entry.context(ScanStoreDirSnafu {
                    path: store_dir.clone(),
                })?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let file_stem = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_owned(),
                    None => continue,
                };

                if declared_ids.contains(&file_stem) {
                    continue; // Not an orphan
                }

                // This is an orphan — read its metadata
                let contents =
                    std::fs::read_to_string(&path).context(StoreFileReadSnafu { path: &path })?;
                let inert: InertStoreResource =
                    serde_json::from_str(&contents).context(DeserializeSnafu {
                        name: file_stem.clone(),
                    })?;

                let type_name = inert.type_name.clone();

                if let Some(ref tn) = type_name {
                    if let Some(deleter) = self.deleters.get(tn) {
                        log::info!(
                            "orphan detected: '{file_stem}' (type: {tn}), scheduling auto-delete"
                        );

                        // Register orphan in remotes so the DAG can track it
                        let (remote_var, rez, _ty) = self
                            .remotes
                            .dequeue_var::<serde_json::Value>(&file_stem, Action::Destroy)?;
                        remote_var.set(Some(inert.remote.clone()));

                        // Resolve dependency keys for correct ordering.
                        // Use the explicit dependencies field if available,
                        // otherwise fall back to parsing depends_on from JSON.
                        let stored_deps = inert
                            .dependencies
                            .as_ref()
                            .cloned()
                            .unwrap_or_else(|| extract_depends_on_from_json(&inert.local));
                        let dep_keys: Vec<usize> = stored_deps
                            .iter()
                            .filter_map(|dep| self.remotes.get(dep).map(|v| v.key))
                            .collect();

                        // Add a load node to introduce the resource into the DAG
                        let load_node_name = format!("load {file_stem}");
                        let load_node = dagga::Node::new(StoreNode {
                            name: load_node_name.clone(),
                            _remote_ty: "orphan",
                            run: Box::new({
                                let resource_id = file_stem.clone();
                                move |_provider: &P| {
                                    Box::pin(async move {
                                        log::debug!("loading orphan '{resource_id}' for deletion");
                                        Ok(())
                                    })
                                        as Pin<Box<dyn Future<Output = Result<()>> + '_>>
                                }
                            }),
                        })
                        .with_name(load_node_name)
                        .with_reads(dep_keys)
                        .with_result(rez);
                        self.graph.add_node(load_node);

                        // Add the destroy node using the registered deleter
                        let destroy_node_name = format!("destroy {file_stem}");
                        let run_fn = (deleter.make_run_fn)(self.path.clone(), file_stem.clone());
                        let destroy_node = dagga::Node::new(StoreNode {
                            name: destroy_node_name.clone(),
                            _remote_ty: "orphan",
                            run: run_fn,
                        })
                        .with_name(destroy_node_name)
                        .with_move(rez);
                        self.graph.add_node(destroy_node);

                        actions.push(PlannedAction {
                            id: file_stem,
                            action: Action::Destroy,
                            type_name: type_name.clone(),
                            is_orphan: true,
                        });

                        continue;
                    }
                }

                // Can't auto-delete: the resource type wasn't used in this run
                // and wasn't manually registered, so we don't have a deleter.
                let msg = match &type_name {
                    Some(tn) => format!(
                        "Orphaned resource '{file_stem}' (type: {tn}) found in the store \
                        directory but its type is not known to this run. Call \
                        `store.register::<{tn}>()` to enable automatic deletion, or use \
                        `store.destroy::<{tn}>(\"{file_stem}\")` to delete it explicitly."
                    ),
                    None => format!(
                        "Orphaned resource '{file_stem}' found in the store directory but \
                        its store file has no type_name. Use \
                        `store.destroy(\"{file_stem}\")` to delete it explicitly."
                    ),
                };
                log::warn!("{msg}");
                warnings.push(msg);
            }
        }

        // Build the schedule from the DAG
        let graph = std::mem::take(&mut self.graph);
        let schedule = graph
            .build_schedule()
            .map_err(|e| Error::Schedule { msg: e.to_string() })?;

        // Reorder actions to match the schedule's execution order.
        // Node names are "{action} {id}" (e.g. "create bucket"). We extract
        // the resource ID and use the first occurrence in schedule order as
        // the canonical position for that action.
        let mut ordered_actions = Vec::with_capacity(actions.len());
        let mut seen = std::collections::HashSet::new();
        for batch in &schedule.batches {
            for node in batch {
                let id = node
                    .name()
                    .split_once(' ')
                    .map(|(_, id)| id)
                    .unwrap_or(node.name());
                if seen.insert(id.to_owned()) {
                    if let Some(pos) = actions.iter().position(|a| a.id == id) {
                        ordered_actions.push(actions.swap_remove(pos));
                    }
                }
            }
        }
        // Append any remaining actions not found in the schedule
        ordered_actions.extend(actions);
        let actions = ordered_actions;

        Ok(Plan {
            actions,
            warnings,
            schedule,
        })
    }

    /// Execute a plan previously built by [`Store::plan`].
    pub async fn apply(&mut self, plan: Plan<P>) -> Result<()> {
        for (i, batch) in plan.schedule.batches.into_iter().enumerate() {
            for (j, node) in batch.into_iter().enumerate() {
                log::debug!("applying node {j}, batch {i}");
                let store_node = node.into_inner();
                (store_node.run)(&self.provider).await?;
            }
        }
        Ok(())
    }

    /// Acknowledge an orphaned resource and prepare it for migration.
    ///
    /// Use this when removing a resource that other resources still depend
    /// on. The returned [`DestroyResource`] provides
    /// [`DestroyResource::migrate`] for extracting values into
    /// [`Migrated`](remote::Migrated) fields on other resources.
    ///
    /// This has the same effect as [`Store::destroy`] but communicates the
    /// intent of handling an orphan in a plan/apply workflow.
    pub fn pending_destroy<T>(&mut self, id: impl AsRef<str>) -> Result<DestroyResource<T>, Error>
    where
        T: Resource<Provider = P>,
    {
        self.destroy(id)
    }
}
