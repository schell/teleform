//! Example: managing a fictional "website hosting" infrastructure with
//! teleform.
//!
//! Resources are backed by ordinary files and directories on the local
//! filesystem, standing in for real cloud resources. Run with
//! `RUST_LOG=info` to see what teleform does under the hood.
//!
//! ```sh
//! cargo run -p teleform-example -- apply
//! cargo run -p teleform-example -- apply --title "Updated"
//! cargo run -p teleform-example -- plan
//! cargo run -p teleform-example -- destroy
//! ```

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use tele::remote::Remote;
use tele::{HasDependencies, Plan, Resource, Store};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "infra", about = "Manage a fictional website's infrastructure")]
struct Cli {
    /// Directory for teleform state files.
    #[arg(long, default_value = "state")]
    state_dir: PathBuf,

    /// Directory where "infrastructure" is written.
    #[arg(long, default_value = "infra")]
    infra_dir: PathBuf,

    /// Name of the website / bucket.
    #[arg(long, default_value = "my-site")]
    site_name: String,

    /// Title shown on the HTML page.
    #[arg(long, default_value = "Hello Teleform")]
    title: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show what would change without applying.
    Plan,
    /// Plan and apply infrastructure changes.
    Apply,
    /// Tear down all infrastructure.
    Destroy {
        #[clap(long, short, default_value = "false")]
        force: bool,
    },
}

// ---------------------------------------------------------------------------
// Resource types
// ---------------------------------------------------------------------------

/// A directory that represents a storage bucket.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Bucket {
    name: String,
}

impl HasDependencies for Bucket {}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct BucketInfo {
    path: String,
}

impl Resource for Bucket {
    /// The provider carries the infrastructure output directory.
    type Provider = PathBuf;
    type Error = String;
    type Output = BucketInfo;

    async fn create(&self, infra_dir: &Self::Provider) -> Result<Self::Output, Self::Error> {
        let dir = infra_dir.join(&self.name);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        log::info!("  created directory {dir:?}");
        Ok(BucketInfo {
            path: dir.to_string_lossy().into_owned(),
        })
    }

    async fn read(&self, provider: &Self::Provider) -> Result<Self::Output, Self::Error> {
        self.create(provider).await
    }

    async fn update(
        &self,
        provider: &Self::Provider,
        _previous_local: &Self,
        _previous_remote: &Self::Output,
    ) -> Result<Self::Output, Self::Error> {
        self.create(provider).await
    }

    async fn delete(
        &self,
        infra_dir: &Self::Provider,
        _previous_remote: &Self::Output,
    ) -> Result<(), Self::Error> {
        let dir = infra_dir.join(&self.name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
            log::info!("  removed directory {dir:?}");
        }
        Ok(())
    }
}

/// An HTML page written into a bucket.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, HasDependencies)]
struct HtmlPage {
    title: String,
    filename: String,
    bucket_path: Remote<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct PageInfo {
    url: String,
}

impl Resource for HtmlPage {
    type Provider = PathBuf;
    type Error = String;
    type Output = PageInfo;

    async fn create(&self, _provider: &Self::Provider) -> Result<Self::Output, Self::Error> {
        let bucket_path = self.bucket_path.get().map_err(|e| e.to_string())?;
        let file = Path::new(&bucket_path).join(&self.filename);
        let html = format!(
            "<html>\n<head><title>{title}</title></head>\n\
             <body>\n  <h1>{title}</h1>\n  \
             <p>Deployed with teleform.</p>\n</body>\n</html>\n",
            title = self.title,
        );
        std::fs::write(&file, html).map_err(|e| e.to_string())?;
        log::info!("  wrote {file:?}");
        let url = format!("file://{}", file.display());
        Ok(PageInfo { url })
    }

    async fn read(&self, provider: &Self::Provider) -> Result<Self::Output, Self::Error> {
        self.create(provider).await
    }

    async fn update(
        &self,
        provider: &Self::Provider,
        _previous_local: &Self,
        _previous_remote: &Self::Output,
    ) -> Result<Self::Output, Self::Error> {
        self.create(provider).await
    }

    async fn delete(&self, _provider: &Self::Provider, _prev: &Self::Output) -> Result<(), String> {
        let bucket_path = self.bucket_path.get().map_err(|e| e.to_string())?;
        let file = Path::new(&bucket_path).join(&self.filename);
        if file.exists() {
            std::fs::remove_file(&file).map_err(|e| e.to_string())?;
            log::info!("  removed {file:?}");
        }
        Ok(())
    }
}

/// A JSON deploy manifest that references the bucket and the page.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, HasDependencies)]
struct DeployManifest {
    site_name: String,
    bucket_path: Remote<String>,
    page_url: Remote<String>,
    // page_two_url: Option<Remote<String>>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct ManifestInfo {
    manifest_path: String,
}

impl Resource for DeployManifest {
    type Provider = PathBuf;
    type Error = String;
    type Output = ManifestInfo;

    async fn create(&self, _provider: &Self::Provider) -> Result<Self::Output, Self::Error> {
        let bucket_path = self.bucket_path.get().map_err(|e| e.to_string())?;
        let page_url = self.page_url.get().map_err(|e| e.to_string())?;
        let file = Path::new(&bucket_path).join("manifest.json");
        let manifest = serde_json::json!({
            "site": self.site_name,
            "url": page_url,
        });
        let contents = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        std::fs::write(&file, contents).map_err(|e| e.to_string())?;
        log::info!("  wrote {file:?}");
        Ok(ManifestInfo {
            manifest_path: file.to_string_lossy().into_owned(),
        })
    }

    async fn read(&self, provider: &Self::Provider) -> Result<Self::Output, Self::Error> {
        self.create(provider).await
    }

    async fn update(
        &self,
        provider: &Self::Provider,
        _previous_local: &Self,
        _previous_remote: &Self::Output,
    ) -> Result<Self::Output, Self::Error> {
        self.create(provider).await
    }

    async fn delete(
        &self,
        _provider: &Self::Provider,
        _previous_remote: &Self::Output,
    ) -> Result<(), Self::Error> {
        let bucket_path = self.bucket_path.get().map_err(|e| e.to_string())?;
        let file = Path::new(&bucket_path).join("manifest.json");
        if file.exists() {
            std::fs::remove_file(&file).map_err(|e| e.to_string())?;
            log::info!("  removed {file:?}");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Infrastructure declaration
// ---------------------------------------------------------------------------

fn declare_infra(
    store: &mut Store<PathBuf>,
    site_name: &str,
    title: &str,
) -> Result<(), tele::Error> {
    let bucket = store.resource(
        "bucket",
        Bucket {
            name: site_name.to_owned(),
        },
    )?;

    let page = store.resource(
        "page",
        HtmlPage {
            title: title.to_owned(),
            filename: "index.html".to_owned(),
            bucket_path: bucket.remote(|b| b.path.clone()),
        },
    )?;

    // let page_two = store.resource(
    //     "page-two",
    //     HtmlPage {
    //         title: "Page Two".to_owned(),
    //         filename: "two.html".to_owned(),
    //         bucket_path: bucket.remote(|b| b.path.clone()),
    //     },
    // )?;

    let _manifest = store.resource(
        "manifest",
        DeployManifest {
            site_name: site_name.to_owned(),
            bucket_path: bucket.remote(|b| b.path.clone()),
            page_url: page.remote(|p| p.url.clone()),
            // page_two_url: None, //Some(page_two.remote(|p| p.url.clone())),
        },
    )?;

    Ok(())
}

fn print_plan(plan: &Plan<PathBuf>) {
    println!("Plan: {plan}");
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let cli = Cli::parse();

    let mut store = Store::new(&cli.state_dir, cli.infra_dir.clone());
    declare_infra(&mut store, &cli.site_name, &cli.title)?;
    let plan = store.plan()?;

    match cli.command {
        Command::Plan => {
            println!("Plan: {plan}");
        }
        Command::Apply => {
            println!("Plan: {plan}");
            println!();
            println!("Applying...");
            store.apply(plan).await?;
            println!("Done.");
        }
        Command::Destroy { force } => {
            // Destroy in reverse dependency order: manifest, page, bucket.
            // Resources with no active `resource()` call will be detected as
            // orphans and auto-deleted (resource types are registered
            // automatically when first used via `store.resource()`).
            store.clear_resources();
            let plan = store.plan()?;
            print_plan(&plan);
            if force {
                println!();
                println!("Applying...");
                store.apply(plan).await?;
                println!("Done.");
            } else {
                println!();
                println!("Please call `destroy --force` to delete these resources.");
            }
        }
    }
    Ok(())
}
