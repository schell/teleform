//! # Teleform
//! The async DAG resource runner.
use std::{future::Future, ops::Deref, pin::Pin};

use dagga::{dot::DagLegend, Node, Schedule};
use snafu::prelude::*;

pub mod remote;
use remote::Remote;
use tokio::io::AsyncWriteExt;

use crate::v2::remote::{Migrated, RemoteVar, Remotes};

pub trait UserError: core::fmt::Display + core::fmt::Debug + 'static {}
impl<T: core::fmt::Display + core::fmt::Debug + 'static> UserError for T {}

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

    #[snafu(display("Missing previous remote value '{name}'"))]
    Load { name: String },

    #[snafu(display("Could not downcast"))]
    Downcast,

    #[snafu(display("Missing store file for '{id}'"))]
    MissingStoreFile { id: String },
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

pub trait Resource:
    Clone + PartialEq + HasDependencies + serde::Serialize + serde::de::DeserializeOwned + 'static
{
    /// Type of the platform/resource provider.
    ///
    /// For example `SdkConfig` in the case of amazon web services.
    type Provider;

    /// Errors that may occur interacting with the provider.
    type Error: UserError;

    /// The remote type of this resource, which we can used to fill in
    /// [`Remote`] values in other resources.
    type Output: core::fmt::Debug + Clone + serde::Serialize + serde::de::DeserializeOwned + 'static;

    fn create(
        &self,
        provider: &Self::Provider,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>>;

    fn update(
        &self,
        provider: &Self::Provider,
        previous_local: &Self,
        previous_remote: &Self::Output,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>>;

    fn delete(
        &self,
        provider: &Self::Provider,
        previous_remote: &Self::Output,
    ) -> impl Future<Output = Result<(), Self::Error>>;
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

pub trait HasDependencies {
    fn dependencies(&self) -> Dependencies;
}

/// `Create`, `Load` and `Update` result in a resource being added to the graph.
///
/// `Store` and `Destroy` move the resource out of the graph
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Action {
    Create,
    Load,
    Update,
    Store,
    Destroy,
}

impl core::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Action::Create => "create",
            Action::Load => "load",
            Action::Update => "update",
            Action::Store => "store",
            Action::Destroy => "destroy",
        })
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct InertStoreResource {
    name: String,
    local: serde_json::Value,
    remote: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct StoreResource<L, R> {
    /// Name of the resource from the user's perspective
    name: String,
    /// Local definition in _code_
    local_definition: L,

    remote_var: RemoteVar<R>,
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
        })
    }
}

/// The path to an individual resource store file.
fn store_file_path(name: &str, store_path: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    store_path.as_ref().join(format!("{name}.json"))
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
    resource_id: &'static str,
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

        match action {
            Action::Create => {
                let value = local_definition_code
                    .create(provider)
                    .await
                    .map_err(|error| Error::Create {
                        name: resource_id.to_owned(),
                        error: Box::new(error),
                    })?;
                remote_var.set(Some(value));
            }
            Action::Load => {}
            Action::Update => {
                let previous_local = local_definition_store.unwrap();
                let previous_remote = remote_var.get().context(LoadSnafu { name: resource_id })?;
                let output = local_definition_code
                    .update(provider, &previous_local, &previous_remote)
                    .await
                    .map_err(|error| Error::Update {
                        name: resource_id.to_owned(),
                        error: Box::new(error),
                    })?;
                remote_var.set(Some(output));
            }
            Action::Destroy => {
                log::debug!("running destroy action on {resource_id}");
                // In the destroy case there is no code-local definition, but there is always
                // a store definition, so we pass the store definition as the code definition.
                // This is better IMO than having both code-local and store be optional.
                let local_definition = local_definition_code.clone();
                let previous_remote = remote_var.get().context(LoadSnafu { name: resource_id })?;
                local_definition
                    .delete(provider, &previous_remote)
                    .await
                    .map_err(|error| Error::Destroy {
                        name: resource_id.to_owned(),
                        error: Box::new(error),
                    })?;

                log::info!("  {resource_id} is destroyed");
                let path = store_file_path(resource_id, &store_path);
                log::info!("  removing {resource_id} store file {path:?}");
                tokio::fs::remove_file(&path)
                    .await
                    .context(StoreFileDeleteSnafu { path })?;
                remote_var.set(None);
            }
            Action::Store => {
                let path = store_file_path(resource_id, &store_path);
                log::info!("storing {resource_id} to {path:?}");
                let inert_resource = InertStoreResource {
                    name: resource_id.to_owned(),
                    local: serde_json::to_value(local_definition_code).context(SerializeSnafu {
                        name: format!("store {resource_id}"),
                    })?,
                    remote: serde_json::to_value(
                        remote_var.get().context(LoadSnafu { name: resource_id })?,
                    )
                    .context(SerializeSnafu {
                        name: format!("store {resource_id} remote"),
                    })?,
                };
                let contents =
                    serde_json::to_string_pretty(&inert_resource).context(SerializeSnafu {
                        name: format!("storing {}", resource_id),
                    })?;

                let mut file = tokio::fs::File::create(&path)
                    .await
                    .context(CreateFileSnafu { path: path.clone() })?;
                file.write_all(contents.as_bytes())
                    .await
                    .context(WriteFileSnafu { path: path.clone() })?;
            }
        }

        log::info!("  success!");
        Ok(())
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
        f: fn(&T::Output) -> X,
    ) -> Remote<T, X> {
        Remote::new(self, f)
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

pub struct Store<T> {
    path: std::path::PathBuf,
    provider: T,
    remotes: Remotes,
    graph: dagga::Dag<StoreNode<T>, usize>,
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
        }
    }

    fn read<T>(&self, id: &'static str) -> Result<(T, T::Output), Error>
    where
        T: Resource<Provider = P>,
    {
        Self::read_from_store(&self.path, id)
    }

    /// Defines a resource.
    ///
    /// Produces two graph nodes, one of `Create | Load | Update` and then a `Store`.
    pub fn resource<T>(
        &mut self,
        id: &'static str,
        local_definition: T,
    ) -> Result<StoreResource<T, T::Output>, Error>
    where
        T: Resource<Provider = P>,
    {
        let (action, stored_definition, output) =
            if let Ok((stored_definition, output)) = self.read(id) {
                // This has already been created and stored, so this is either a simple load,
                // or an update.
                log::debug!("  {output:?}");
                let action = if local_definition != stored_definition {
                    log::debug!("  local resource has changed, so this remote is now stale");
                    Action::Update
                } else {
                    Action::Load
                };

                (action, Some(stored_definition), Some(output))
            } else {
                log::debug!("creating an empty '{id}'");
                (Action::Create, None, None)
            };

        let (remote_var, rez, _ty) = self.remotes.dequeue_var::<T::Output>(id, action)?;
        remote_var.set(output);

        let remote_var = remote_var.clone();
        let local_definition_code = local_definition.clone();
        let local_definition_store = stored_definition.clone();
        let store_path = self.path.clone();
        let run: StoreNodeRunFn<T::Provider> = Box::new({
            let remote_var = remote_var.clone();
            let local_definition_code = local_definition_code.clone();
            let local_definition_store = local_definition_store.clone();
            move |provider: &T::Provider| {
                Box::pin(
                    RunAction {
                        provider,
                        store_path,
                        resource_id: id,
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
                Action::Create | Action::Load | Action::Update => {
                    log::debug!("  with result {rez}");
                    dag_node.with_result(rez)
                }
                Action::Store | Action::Destroy => {
                    log::debug!("  with move {rez}");
                    dag_node.with_move(rez)
                }
            };
            self.graph.add_node(dag_node);
        }

        // This if is technically unneccessary since the function never creates a
        // destory action, but it illustrates the point.
        if action != Action::Destroy {
            log::debug!("adding secondary node {} {id}", Action::Store);
            let node_name = format!("store {id}");
            let storage_node = dagga::Node::new(StoreNode {
                name: node_name.clone(),
                _remote_ty: std::any::type_name::<T>(),
                run: Box::new({
                    let store_path = self.path.clone();
                    let remote_var = remote_var.clone();
                    move |provider| {
                        Box::pin(
                            RunAction {
                                provider,
                                store_path,
                                resource_id: id,
                                action: Action::Store,
                                local_definition_code,
                                local_definition_store,
                                remote_var,
                            }
                            .run(),
                        )
                    }
                }),
            })
            .with_name(node_name)
            .with_move(rez);
            self.graph.add_node(storage_node);
        }

        Ok(StoreResource {
            name: id.to_owned(),
            local_definition,
            remote_var,
        })
    }

    /// Destroys a resource.
    pub fn destroy<T>(&mut self, id: &'static str) -> Result<DestroyResource<T>, Error>
    where
        T: Resource<Provider = P>,
    {
        let (local, remote) = self.read::<T>(id)?;
        let (remote_var, rez, _ty) = self.remotes.dequeue_var::<T::Output>(id, Action::Destroy)?;
        {
            // Destruction requires a load to introduce the resource (for the DAG)
            log::debug!("adding node {} {id}", Action::Load);
            let node_name = format!("load {id}");
            let load_node = dagga::Node::new(StoreNode {
                name: node_name.clone(),
                _remote_ty: std::any::type_name::<T>(),
                run: Box::new({
                    let store_path = self.path.clone();
                    let local = local.clone();
                    let remote_var = remote_var.clone();
                    move |provider| {
                        Box::pin(
                            RunAction {
                                provider,
                                store_path,
                                resource_id: id,
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
                    let local = local.clone();
                    let store_path = self.path.clone();
                    let remote_var = remote_var.clone();
                    move |provider| {
                        Box::pin(
                            RunAction {
                                provider,
                                store_path,
                                resource_id: id,
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
                .with_moves(node.get_moves().copied());
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

    pub async fn apply(&mut self) -> Result<()> {
        let graph = std::mem::take(&mut self.graph);
        let schedule = graph
            .build_schedule()
            .map_err(|e| Error::Schedule { msg: e.to_string() })?;
        for (i, batch) in schedule.batches.into_iter().enumerate() {
            for (j, node) in batch.into_iter().enumerate() {
                log::debug!("applying node {j}, batch {i}");
                let store_node = node.into_inner();
                (store_node.run)(&self.provider).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test;
