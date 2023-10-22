//! # Teleform
//! Like Terraform, but Rusty.
use anyhow::Context;
use colored::Colorize;
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    future::Future,
    ops::{Deref, DerefMut},
    pin::Pin,
};

pub use teleform_derive::TeleSync;
pub mod aws;

/// A remote infrastructure resource.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rez {
    pub type_is: Option<String>,
    pub data: serde_json::Value,
    #[serde(skip_serializing, skip_deserializing)]
    use_count: usize,
}

impl PartialOrd for Rez {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.type_is.partial_cmp(&other.type_is) {
            Some(Ordering::Equal) => {}
            ord => return ord,
        }
        let data_here = self.data.to_string();
        let data_there = other.data.to_string();
        data_here.partial_cmp(&data_there)
    }
}

impl Rez {
    pub fn new<T: std::any::Any + serde::Serialize>(data: T) -> anyhow::Result<Self> {
        Ok(Self {
            type_is: Some(std::any::type_name::<T>().to_string()),
            data: serde_json::to_value(data)?,
            use_count: 0,
        })
    }
}

/// Pick between two values.
///
/// This is used to solve the "remote values problem" (see DEVLOG.md).
///
/// ## tl;dr
/// Some values are determined after resource creation, but we don't
/// want to update the callsite (the IaC definition) with an explicit
/// value, so we need the callsite to become a composite of the
/// IaC definition and what's in the store file.
pub trait TeleEither: Sized {
    #[allow(unused_variables)]
    fn either(self, other: Self) -> Self {
        self
    }
}

impl<T: TeleEither> TeleEither for Option<T> {
    fn either(self, other: Self) -> Self {
        match (self, other) {
            (Some(a), Some(b)) => Some(a.either(b)),
            (a, _) => a,
        }
    }
}

/// A local value, known before resource creation.
#[derive(Debug, Default, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Local<T>(pub T);

/// The `TeleEither` implementation for Local always picks itself.
impl<T> TeleEither for Local<T> {}

impl<T> AsRef<T> for Local<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl<T> AsMut<T> for Local<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T> Deref for Local<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Local<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> From<T> for Local<T> {
    fn from(value: T) -> Self {
        Local(value)
    }
}

impl<'a> Into<Local<String>> for &'a str {
    fn into(self) -> Local<String> {
        Local::from(self.to_string())
    }
}

/// A remote value, only known after resource creation.
#[derive(Debug, Default, PartialEq, Clone)]
pub enum Remote<T> {
    #[default]
    Unknown,
    Remote(T),
}

/// The `TeleEither` implementation for `Remote` picks the other if `self` is unknown.
impl<T> TeleEither for Remote<T> {
    fn either(self, other: Self) -> Self {
        if matches!(self, Remote::Unknown) {
            other
        } else {
            self
        }
    }
}

impl<T> From<T> for Remote<T> {
    fn from(value: T) -> Self {
        Remote::Remote(value)
    }
}

impl<T: serde::Serialize> serde::Serialize for Remote<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Remote::Unknown => serializer.serialize_none(),
            Remote::Remote(s) => s.serialize(serializer),
        }
    }
}

impl<'de, T: serde::de::Deserialize<'de>> serde::de::Deserialize<'de> for Remote<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(T::deserialize(deserializer)
            .map(Remote::Remote)
            .unwrap_or(Remote::Unknown))
    }
}

impl<T: std::fmt::Display> std::fmt::Display for Remote<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Remote::Unknown => f.write_str("*unknown*"),
            Remote::Remote(t) => t.fmt(f),
        }
    }
}

impl<T> Remote<T> {
    pub fn maybe_ref(&self) -> Option<&T> {
        match self {
            Remote::Unknown => None,
            Remote::Remote(s) => Some(s),
        }
    }

    #[allow(dead_code)]
    pub fn is_known(&self) -> bool {
        match self {
            Remote::Remote(_) => true,
            _ => false,
        }
    }
}

/// Synchronize an IaC definition with a stored type, mutating infrastructure to match.
pub trait TeleSync
where
    Self: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
{
    type Provider;

    fn composite(self, other: Self) -> Self;

    fn should_recreate(&self, other: &Self) -> bool;

    fn should_update(&self, other: &Self) -> bool;

    fn create<'a>(
        &'a mut self,
        apply: bool,
        helper: &'a Self::Provider,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>>;

    fn update<'a>(
        &'a mut self,
        apply: bool,
        helper: &'a Self::Provider,
        name: &'a str,
        previous: &'a Self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>>;

    fn delete<'a>(
        &'a self,
        apply: bool,
        helper: &'a Self::Provider,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>>;
}

/// An IaC store.
#[derive(Debug)]
pub struct Store<Config> {
    path: std::path::PathBuf,
    pub apply: bool,
    pub cfg: Config,
    rez: BTreeMap<String, Rez>,
}

impl<Config> Store<Config> {
    pub fn path(&self) -> &std::path::PathBuf {
        &self.path
    }

    /// Insert an IaC resource into the store.
    ///
    /// This is useful for adding resources created outside of teleform.
    pub fn insert<'a, Data>(&mut self, name: impl Into<String>, data: Data) -> anyhow::Result<()>
    where
        Config: AsRef<<Data as TeleSync>::Provider>,
        Data: std::any::Any + TeleSync,
    {
        let name = name.into();
        let json = serde_json::to_string_pretty(&data)?;
        log::info!("inserting {name}:\n{json}");
        let entry = self.rez.entry(name).or_default();
        entry.type_is = Some(std::any::type_name::<Data>().to_string());
        entry.data = serde_json::to_value(&data)?;
        entry.use_count += 1;
        Ok(())
    }

    /// Synchronize a singular IaC resource.
    pub async fn sync<'a, Data>(
        &mut self,
        name: impl Into<String>,
        mut data: Data,
    ) -> anyhow::Result<Data>
    where
        Config: AsRef<<Data as TeleSync>::Provider>,
        Data: std::any::Any + TeleSync + Clone,
    {
        use colored::*;

        let name = name.into();
        let provider: &Data::Provider = self.cfg.as_ref();
        log::trace!("sync'ing {name}");
        if let Some(existing) = self.rez.get_mut(&name) {
            let existing_data: Data = serde_json::from_value(existing.data.clone())
                .with_context(|| format!("could not deserialize {name}"))?;
            data = data.composite(existing_data.clone());
            // UNWRAP: safe because rez always serializes
            let prev = serde_json::to_string_pretty(&existing).unwrap();
            let new = {
                let mut new_rez = existing.clone();
                new_rez.data = serde_json::to_value(data.clone())?;
                serde_json::to_string_pretty(&new_rez)?
            };
            let comparison = pretty_assertions::StrComparison::new(&prev, &new);
            // recreate or update
            if existing_data.should_recreate(&data) {
                log::info!("recreating {name}:\n{comparison}");
                log::info!("deleting {name}");
                data.delete(self.apply, provider, &name).await?;
                if self.apply {
                    log::info!("...deleted");
                }
                log::info!("creating {name}");
                data.create(self.apply, provider, &name).await?;
                if self.apply {
                    log::info!("...created");
                }
            } else if existing_data.should_update(&data) {
                log::info!("updating {name}:\n{comparison}");
                data.update(self.apply, provider, &name, &existing_data)
                    .await?;
                if self.apply {
                    log::info!("...updated");
                }
            } else {
                data = existing_data;
            }
            existing.type_is = Some(std::any::type_name::<Data>().to_string());
            existing.data = serde_json::to_value(data.clone())?;
            existing.use_count += 1;
        } else {
            // create
            log::info!(
                "creating {name}:\n{}",
                serde_json::to_string_pretty(&data).context("json")?.green()
            );
            data.create(self.apply, provider, &name).await?;
            if self.apply {
                log::info!("...created");
            }
            let mut rez = Rez::new(data.clone())?;
            rez.use_count += 1;
            self.rez.insert(name, rez);
        };
        if self.apply {
            self.save(&self.path)?;
        }
        Ok(data)
    }

    pub fn from_path(
        apply: bool,
        cfg: Config,
        path: impl AsRef<std::path::Path>,
    ) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = std::fs::File::open(&path)?;
        let rez: BTreeMap<String, Rez> = serde_json::from_reader(file)?;
        Ok(Store {
            path,
            apply,
            cfg,
            rez,
        })
    }

    pub fn empty(apply: bool, cfg: Config) -> Self {
        Store {
            path: std::env::current_dir().unwrap().join("default_store.json"),
            apply,
            cfg,
            rez: Default::default(),
        }
    }

    pub fn get_prunes(&self) -> Vec<String> {
        self.rez
            .iter()
            .filter_map(|(name, rez)| {
                if rez.use_count == 0 {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    }

    pub async fn prune<Data>(&mut self) -> anyhow::Result<()>
    where
        Config: AsRef<Data::Provider>,
        Data: TeleSync,
    {
        let to_prune = self.get_prunes();
        if !to_prune.is_empty() {
            for name in to_prune.into_iter() {
                if let Some(rez) = self.rez.get(&name) {
                    let type_is = Some(std::any::type_name::<Data>().to_string());
                    if rez.type_is == type_is {
                        // UNWRAP: safe because we just created it above
                        log::warn!("cleaning up resource {name} {}", type_is.unwrap());
                        // UNWRAP: safe because Value always converts
                        log::info!("{}", serde_json::to_string_pretty(&rez.data).unwrap().red());
                    } else {
                        continue;
                    }
                }
                // UNWRAP: safe because we just got this rez from the store, or we would
                // have `continue`d above
                let rez = self.rez.remove(&name).unwrap();
                match serde_json::from_value::<Data>(rez.data.clone()) {
                    Ok(data) => {
                        if self.apply {
                            data.delete(self.apply, self.cfg.as_ref(), &name).await?;
                            self.save(&self.path)?;
                            log::info!("...deleted");
                        }
                    }
                    Err(_) => {
                        self.rez.insert(name, rez);
                    }
                }
            }
        }

        Ok(())
    }

    /// Delete the resource with the given name, if any.
    pub async fn _delete<Data>(&mut self, name: impl Into<String>) -> anyhow::Result<()>
    where
        Config: AsRef<Data::Provider>,
        Data: TeleSync,
    {
        let name = name.into();
        if let Some(rez) = self.rez.remove(&name) {
            let data: Data = serde_json::from_value(rez.data)?;
            data.delete(self.apply, self.cfg.as_ref(), &name).await?;
        } else {
            log::warn!("cannot delete {name} - no such resource");
        }
        Ok(())
    }

    pub fn save(&self, path: impl AsRef<std::path::Path>) -> anyhow::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(&self.rez)?)?;
        Ok(())
    }
}

pub mod cli {
    //! Utility functions for adding teleform IaC to your command line program.
    //!
    //! This is useful for setting up your infrastructure as a subcommand of xtask,
    //! for example.

    use std::io::Read;

    use anyhow::Context;

    use crate::Store;

    /// Attempt to find the cargo workspace directory by searching for Cargo.lock,
    /// recursively up the filesystem tree.
    pub fn find_workspace_dir() -> anyhow::Result<std::path::PathBuf> {
        let mut workspace_dir = std::env::current_dir()?;
        while !workspace_dir.join("Cargo.lock").is_file() {
            let parent = workspace_dir
                .parent()
                .context("hit root dir while looking for the workspace!")?;
            workspace_dir = parent.to_path_buf();
        }
        Ok(workspace_dir)
    }

    /// Create the infrastructure store, backed by a local file.
    pub fn create_store<Cfg>(
        store_path: impl AsRef<std::path::Path>,
        backup_store_path: impl AsRef<std::path::Path>,
        cfg: Cfg,
        apply: bool,
    ) -> anyhow::Result<Store<Cfg>> {
        let store: Store<Cfg> = if store_path.as_ref().exists() {
            log::debug!(
                "found store file - exists at: {}",
                store_path.as_ref().display()
            );
            Store::from_path(apply, cfg, store_path.as_ref()).context("cannot open store json")?
        } else {
            log::debug!("creating a new empty store");
            Store::empty(apply, cfg)
        };
        if apply {
            log::debug!("backing up to {}", backup_store_path.as_ref().display());
            store.save(backup_store_path.as_ref())?;
        }
        Ok(store)
    }

    /// Display any resources that should be pruned.
    /// Return whether there are resources to prune.
    pub fn display_prunes<Cfg>(store: &Store<Cfg>) -> bool {
        let unused_resources = store.get_prunes();
        if !unused_resources.is_empty() {
            log::warn!(
                "will prune unused resources: \n{}",
                unused_resources
                    .iter()
                    .map(|n| format!("  {n}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            true
        } else {
            log::info!("no resources to prune/delete");
            false
        }
    }

    /// Get confirmation to perform a delete command
    pub fn confirm_delete(confirmation_message: &str) -> anyhow::Result<bool> {
        log::info!("confirm deletion by entering \"{confirmation_message}\":");
        let mut typing = String::new();
        std::io::stdin().read_line(&mut typing)?;
        if typing != format!("{confirmation_message}\n") {
            log::info!("got \"{typing}\", so we're bailing");
            return Ok(false);
        }
        Ok(true)
    }

    /// Returns the sha256 digest of the file at the given path *if it exists*.
    /// If the file does _not_ exist it returns `Ok(None)`.
    pub fn sha256_digest(path: impl AsRef<std::path::Path>) -> anyhow::Result<Option<String>> {
        log::trace!("determining sha256 of {}", path.as_ref().display());
        if !path.as_ref().exists() {
            return Ok(None);
        }

        fn sha256<R: Read>(mut reader: R) -> anyhow::Result<ring::digest::Digest> {
            let mut context = ring::digest::Context::new(&ring::digest::SHA256);
            let mut buffer = [0; 1024];

            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                context.update(&buffer[..count]);
            }

            Ok(context.finish())
        }

        let input = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(input);
        let digest = sha256(reader)?;
        Ok(Some(data_encoding::HEXUPPER.encode(digest.as_ref())))
    }
}
