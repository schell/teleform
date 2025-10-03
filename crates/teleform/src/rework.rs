//! # Teleform
//! The async DAG resource runner.
use std::{future::Future, pin::Pin};

use dagga::{dot::DagLegend, Node, Schedule};
use snafu::prelude::*;

pub mod remote;
use remote::Remote;
use tokio::io::AsyncWriteExt;

use crate::rework::remote::{RemoteVar, Remotes, Status};

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

    #[snafu(display("Could not serialize stored '{name}': {source}"))]
    Serialize {
        name: String,
        source: toml::ser::Error,
    },

    #[snafu(display("Could not deserialize stored '{name}': {source}"))]
    Deserialize {
        name: String,
        source: toml::de::Error,
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

    #[snafu(display("Missing previous remote value '{name}'"))]
    Load { name: String },

    #[snafu(display("Could not downcast"))]
    Downcast,
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

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum Action {
    Create,
    Load,
    Update,
    Delete,
}

impl core::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Action::Create => "create",
            Action::Load => "load",
            Action::Update => "update",
            Action::Delete => "delete",
        })
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct InertStoreResource {
    name: String,
    local: toml::Value,
    remote: Option<toml::Value>,
}

#[derive(Clone, Debug)]
pub struct StoreResource<L, R> {
    /// Name of the resource from the user's perspective
    name: String,
    /// Local definition in _code_
    local_definition: L,
    /// Definition stored in the store _file_
    stored_definition: Option<L>,

    remote_var: RemoteVar<R>,
}

impl<L, R> TryFrom<StoreResource<L, R>> for InertStoreResource
where
    L: serde::Serialize + for<'a> serde::Deserialize<'a>,
    R: Clone + serde::Serialize + for<'a> serde::Deserialize<'a>,
{
    type Error = Error;

    fn try_from(value: StoreResource<L, R>) -> std::result::Result<Self, Self::Error> {
        let local = toml::Value::try_from(value.local_definition).context(SerializeSnafu {
            name: value.name.clone(),
        })?;
        let remote = if let Some(output) = value.remote_var.get().ok() {
            let remote = toml::Value::try_from(output).context(SerializeSnafu {
                name: value.name.clone(),
            })?;
            Some(remote)
        } else {
            None
        };
        Ok(Self {
            name: value.name,
            local,
            remote,
        })
    }
}

impl<L, R> StoreResource<L, R>
where
    L: PartialEq,
    R: Clone + core::fmt::Debug,
{
    /// Return the expected application action for this resource.
    pub fn action(&self) -> Action {
        if let Some(stored_definition) = self.stored_definition.as_ref() {
            if stored_definition == &self.local_definition {
                match self.remote_var.get() {
                    Status::None => Action::Create,
                    Status::Ok(_) => Action::Load,
                    Status::Stale(_) => Action::Update,
                }
            } else {
                Action::Update
            }
        } else {
            Action::Create
        }
    }
}

/// The path to an individual resource store file.
fn store_file_path(name: &str, store_path: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    store_path.as_ref().join(format!("{name}.toml"))
}

type StoreNodeRunFn<Provider> = Box<
    dyn FnOnce(
        // Resource platform provider
        &'_ Provider,
    ) -> Pin<Box<dyn Future<Output = Result<InertStoreResource>> + '_>>,
>;

impl<T> StoreResource<T, T::Output>
where
    T: Resource,
    T::Output: Clone,
{
    /// Convert the resource into a graph node to be added to the Store's inner
    /// DAG.
    fn try_into_node(&self) -> Result<StoreNode<T::Provider>> {
        let action = self.action();
        let remote_var = self.remote_var.clone();
        let any_local_definition_code: Box<dyn core::any::Any> =
            Box::new(self.local_definition.clone());
        let any_local_definition_store: Box<dyn core::any::Any> =
            Box::new(self.stored_definition.clone());
        let name = self.name.clone();

        let run: StoreNodeRunFn<T::Provider> = Box::new(move |provider: &T::Provider| {
            let remote_var = remote_var.clone();
            Box::pin(async move {
                log::info!("{action} '{name}':");
                // UNWRAP: safe because we know the type is T as we box-cast it above
                let local_definition_code: T = *any_local_definition_code.downcast().unwrap();
                let local_definition_store: Option<T> =
                    *any_local_definition_store.downcast().unwrap();

                let result_remote_output = match action {
                    Action::Create => {
                        local_definition_code
                            .create(provider)
                            .await
                            .map_err(|error| Error::Create {
                                name: name.clone(),
                                error: Box::new(error),
                            })
                    }
                    Action::Load => remote_var
                        .get()
                        .ok()
                        .context(LoadSnafu { name: name.clone() }),
                    Action::Update => {
                        let previous_local = local_definition_store.unwrap();
                        let result = match remote_var.get() {
                            Status::None => {
                                return LoadSnafu { name: name.clone() }.fail();
                            }
                            Status::Ok(previous_remote) => {
                                local_definition_code
                                    .update(provider, &previous_local, &previous_remote)
                                    .await
                            }
                            Status::Stale(previous_remote) => {
                                local_definition_code
                                    .update(provider, &previous_local, &previous_remote)
                                    .await
                            }
                        };
                        result.map_err(|error| Error::Update {
                            name: name.clone(),
                            error: Box::new(error),
                        })
                    }
                    Action::Delete => todo!("delete"),
                };
                if let Err(e) = result_remote_output.as_ref() {
                    log::error!("  {e}");
                } else {
                    log::info!("  success!");
                }
                let remote_output = result_remote_output?;
                log::debug!("  {remote_output:#?}");
                remote_var.set(Status::Ok(remote_output.clone()));
                let output = toml::Value::try_from(remote_output)
                    .context(SerializeSnafu { name: name.clone() })?;
                Ok(InertStoreResource {
                    local: toml::Value::try_from(local_definition_code).context(
                        SerializeSnafu {
                            name: format!("post-apply local '{name}'"),
                        },
                    )?,
                    remote: Some(output),
                    name,
                })
            })
        });

        Ok(StoreNode {
            action,
            name: self.name.clone(),
            remote_ty: core::any::type_name::<T::Output>(),
            run,
        })
    }

    /// Map a remote value to use in local definitions.
    pub fn remote<X: Clone + core::fmt::Debug + 'static>(
        &self,
        f: fn(&T::Output) -> X,
    ) -> Remote<T, X> {
        Remote::new(self, f)
    }
}

struct StoreNode<Provider> {
    action: Action,
    name: String,
    remote_ty: &'static str,
    run: StoreNodeRunFn<Provider>,
}

pub struct Store<T> {
    path: std::path::PathBuf,
    provider: T,
    remotes: Remotes,
    graph: dagga::Dag<StoreNode<T>, usize>,
}

impl<P: 'static> Store<P> {
    pub fn new(path: impl AsRef<std::path::Path>, provider: P) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            graph: dagga::Dag::default(),
            remotes: Default::default(),
            provider,
        }
    }

    /// Defines a resource.
    pub fn resource<T>(
        &mut self,
        id: &'static str,
        local_definition: T,
    ) -> Result<StoreResource<T, T::Output>, Error>
    where
        T: Resource<Provider = P>,
    {
        let path = store_file_path(id, &self.path);
        let (remote_var, rez, _ty) = self.remotes.get_var::<T::Output>(id)?;
        let store_resource = if path.exists() {
            log::debug!("{path:?} exists, reading '{id}' from it");
            let contents = std::fs::read_to_string(&path).context(StoreFileReadSnafu {
                path: path.to_path_buf(),
            })?;
            let inert_store_rez: InertStoreResource =
                toml::from_str(&contents).context(DeserializeSnafu {
                    name: id.to_owned(),
                })?;
            let stored_definition: T =
                inert_store_rez.local.try_into().context(DeserializeSnafu {
                    name: id.to_owned(),
                })?;
            if let Some(output) = inert_store_rez.remote {
                log::debug!("  reading remote output toml value");
                let remote_value: T::Output = output.try_into().context(DeserializeSnafu {
                    name: format!("remote {id}"),
                })?;
                log::debug!("  {remote_value:?}");
                let status = if local_definition != stored_definition {
                    log::debug!("  local resource has changed, so this remote is now stale");
                    Status::Stale(remote_value)
                } else {
                    Status::Ok(remote_value)
                };
                remote_var.set(status);
            }
            StoreResource {
                name: id.to_owned(),
                local_definition,
                stored_definition: Some(stored_definition),
                remote_var,
            }
        } else {
            log::debug!("creating an empty '{id}'");
            StoreResource {
                name: id.to_owned(),
                local_definition: local_definition.clone(),
                stored_definition: None,
                remote_var,
            }
        };

        let store_node = store_resource.try_into_node()?;
        if !matches!(store_node.action, Action::Load) {
            let node_name = format!("{} {id}", store_node.action);
            self.graph.add_node(
                dagga::Node::new(store_node)
                    .with_name(node_name)
                    .with_reads({
                        // read the resource keys out of "remotes" as dependencies
                        let mut reads = vec![];
                        for dep in store_resource.local_definition.dependencies() {
                            reads.push(self.remotes.get_rez(&dep)?);
                        }
                        reads
                    })
                    .with_result(rez),
            );
        }

        Ok(store_resource)
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
                .with_name(format!("{} {}", store_node.action, store_node.name))
                .with_reads(node.get_reads().copied())
                .with_results(node.get_results().copied());
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
                    f.write_str("--- batch ")?;
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
                let inert_resource = (store_node.run)(&self.provider).await?;

                let contents = toml::to_string_pretty(&inert_resource).context(SerializeSnafu {
                    name: format!("create result of {}", inert_resource.name),
                })?;

                log::info!("  applied {}, writing to file", inert_resource.name);
                let path = store_file_path(&inert_resource.name, &self.path);
                let mut file = tokio::fs::File::create(&path)
                    .await
                    .context(CreateFileSnafu { path: path.clone() })?;
                file.write_all(contents.as_bytes())
                    .await
                    .context(WriteFileSnafu { path: path.clone() })?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn sanity() {
        let _ = env_logger::builder().try_init();

        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct LocalBucket {
            name: String,
        }

        impl HasDependencies for LocalBucket {
            fn dependencies(&self) -> Dependencies {
                Dependencies::default()
            }
        }

        #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
        struct RemoteBucket {
            arn: [u8; 8],
        }

        impl Resource for LocalBucket {
            type Provider = ();

            type Error = String;

            type Output = RemoteBucket;

            async fn create(&self, (): &Self::Provider) -> Result<Self::Output, Self::Error> {
                let mut arn = [0; 8];
                for (slot, c) in arn.as_mut_slice().iter_mut().zip(self.name.chars()) {
                    *slot = u32::from(c) as u8;
                }
                Ok(RemoteBucket { arn })
            }

            async fn update(
                &self,
                provider: &Self::Provider,
                _previous_local: &Self,
                _previous_remote: &Self::Output,
            ) -> Result<Self::Output, Self::Error> {
                self.create(provider).await
            }
        }

        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct LocalService {
            bucket_arn: Remote<LocalBucket, [u8; 8]>,
        }

        impl HasDependencies for LocalService {
            fn dependencies(&self) -> Dependencies {
                self.bucket_arn.depends_on()
            }
        }

        #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
        struct RemoteService {
            service_id: String,
        }

        impl Resource for LocalService {
            type Provider = ();
            type Error = Error;
            type Output = RemoteService;

            async fn create(&self, (): &Self::Provider) -> Result<Self::Output, Self::Error> {
                let bucket_arn = self.bucket_arn.get()?;
                Ok(RemoteService {
                    service_id: format!("service-{}", bucket_arn.map(|c| c.to_string()).join("")),
                })
            }

            async fn update(
                &self,
                provider: &Self::Provider,
                _previous_local: &Self,
                _previous_remote: &Self::Output,
            ) -> Result<Self::Output, Self::Error> {
                self.create(provider).await
            }
        }

        fn test_output_path() -> std::path::PathBuf {
            std::path::PathBuf::from(std::env!("CARGO_WORKSPACE_DIR")).join("test_output/sanity")
        }

        async fn write_graph_pdf(store: &mut Store<()>, name: &str) {
            if store.graph.is_empty() {
                log::info!("no graph to write");
                return;
            }
            let dotfile = test_output_path().join(format!("{name}.dot"));
            if let Err(e) = store.save_apply_graph(&dotfile) {
                log::error!("dot graph error: {e}");
                panic!("{e}");
            }

            let pdffile = test_output_path().join(format!("{name}.pdf"));
            let cmd = tokio::process::Command::new("dot")
                .arg("-Tpdf")
                .arg(&dotfile)
                .arg("-o")
                .arg(&pdffile)
                .spawn()
                .unwrap();
            if !cmd.wait_with_output().await.unwrap().status.success() {
                panic!("could not save graph");
            }
            tokio::fs::remove_file(dotfile).await.unwrap();
        }

        async fn run_infra(store: &mut Store<()>, step: &str) {
            log::info!("running infra step {step}");

            let bucket_rez = store
                .resource(
                    "test-bucket",
                    LocalBucket {
                        name: "mybucket".to_owned(),
                    },
                )
                .unwrap();
            let service_a = store
                .resource(
                    "test-service-a",
                    LocalService {
                        bucket_arn: bucket_rez.remote(|bucket| bucket.arn),
                    },
                )
                .unwrap();

            let service_b = store
                .resource(
                    "test-service-b",
                    LocalService {
                        bucket_arn: bucket_rez.remote(|bucket| bucket.arn),
                    },
                )
                .unwrap();

            write_graph_pdf(store, step).await;
            log::info!("running plan: \n{}", store.get_schedule_string().unwrap());
            store.apply().await.unwrap();
        }

        tokio::fs::remove_dir_all(test_output_path()).await.unwrap();
        tokio::fs::create_dir_all(test_output_path()).await.unwrap();
        let mut store = Store::new(test_output_path(), ());
        run_infra(&mut store, "create").await;
        run_infra(&mut store, "read").await;

        async fn run_update(store: &mut Store<()>) {
            log::info!("running infra update ");

            let bucket_rez = store
                .resource(
                    "test-bucket",
                    LocalBucket {
                        name: "mybucket-renamed".to_owned(),
                    },
                )
                .unwrap();
            log::warn!("\n");
            let service_a = store
                .resource(
                    "test-service-a",
                    LocalService {
                        bucket_arn: bucket_rez.remote(|bucket| bucket.arn),
                    },
                )
                .unwrap();

            let service_b = store
                .resource(
                    "test-service-b",
                    LocalService {
                        bucket_arn: bucket_rez.remote(|bucket|
                        // {
                        //     let mut arn = bucket.arn;
                        //     arn[0] = 6;
                        //     arn[1] = 6;
                        //     arn[2] = 6;
                        //     arn
                        // }
                        bucket.arn),
                    },
                )
                .unwrap();

            write_graph_pdf(store, "update").await;
            log::info!("running plan: \n{}", store.get_schedule_string().unwrap());

            let legend = store.get_graph_legend().unwrap();
            assert_eq!(
                2,
                legend.schedule.batches.len(),
                "update should be scheduled into two batches: 1 base update and 2 trickle-down"
            );
            store.apply().await.unwrap();
        }
        run_update(&mut store).await;
    }
}
