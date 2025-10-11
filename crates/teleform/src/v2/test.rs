use crate::v2::{self as tele, *};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct LocalBucket {
    name: String,
}

impl HasDependencies for LocalBucket {
    fn dependencies(&self) -> Dependencies {
        Dependencies::default()
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, HasDependencies)]
struct LocalService {
    bucket_arn: Remote<LocalBucket, [u8; 8]>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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
        Ok(())
    }
}

#[tokio::test]
async fn sanity() {
    let _ = env_logger::builder().try_init();

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
        log::warn!("running infra step {step}");

        let bucket_rez = store
            .resource(
                "test-bucket",
                LocalBucket {
                    name: "mybucket".to_owned(),
                },
            )
            .unwrap();
        let _service_a = store
            .resource(
                "test-service-a",
                LocalService {
                    bucket_arn: bucket_rez.remote(|bucket| bucket.arn),
                },
            )
            .unwrap();

        let _service_b = store
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

    async fn backup(suffix: &str) {
        let mut dir = tokio::fs::read_dir(test_output_path()).await.unwrap();
        while let Some(entry) = dir.next_entry().await.unwrap() {
            if entry.path().is_file() {
                if let Some(ext) = entry.path().extension() {
                    let ext = ext.to_str().unwrap();
                    if ext == "json" {
                        let backup_dir = test_output_path().join(suffix);
                        tokio::fs::create_dir_all(&backup_dir).await.unwrap();
                        tokio::fs::copy(
                            entry.path(),
                            backup_dir.join(entry.path().file_name().unwrap()),
                        )
                        .await
                        .unwrap();
                    }
                }
            }
        }
        log::warn!("\n");
    }

    if test_output_path().exists() {
        tokio::fs::remove_dir_all(test_output_path()).await.unwrap();
    }
    tokio::fs::create_dir_all(test_output_path()).await.unwrap();
    let mut store = Store::new(test_output_path(), ());
    run_infra(&mut store, "create").await;
    backup("create").await;
    run_infra(&mut store, "read").await;
    backup("read").await;
    log::warn!("\n");

    async fn run_update(store: &mut Store<()>) {
        log::warn!("running infra update");

        let bucket_rez = store
            .resource(
                "test-bucket",
                LocalBucket {
                    name: "mybucket-renamed".to_owned(),
                },
            )
            .unwrap();
        log::warn!("\n");
        let _service_a = store
            .resource(
                "test-service-a",
                LocalService {
                    bucket_arn: bucket_rez.remote(|bucket| bucket.arn),
                },
            )
            .unwrap();

        let _service_b = store
            .resource(
                "test-service-b",
                LocalService {
                    bucket_arn: bucket_rez.remote(|bucket| bucket.arn),
                },
            )
            .unwrap();

        write_graph_pdf(store, "update").await;
        log::info!("running plan: \n{}", store.get_schedule_string().unwrap());

        let legend = store.get_graph_legend().unwrap();
        assert_eq!(
            3,
            legend.schedule.batches.len(),
            "update should be scheduled into 3 batches: 1 update and 2 load 3 storage"
        );
        store.apply().await.unwrap();
    }
    run_update(&mut store).await;
    backup("update").await;
    log::warn!("\n");

    // In order to delete the bucket which has downstream dependencies, we must
    // be able to remove the bucket as a dependency from those downstream resources.
    //
    // We can do this by migrating the downstream resources to a new resource type
    // that serializes the same way (or similar enough to be read).
    //
    // In practice we wouldn't have to define a new type for LocalService, though
    // you could if you wanted. It would be perfectly fine to simply edit the
    // struct definition in place.
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct LocalService2 {
        // Here we got rid of the `Remote`
        bucket_arn: Migrated<[u8; 8]>,
    }

    impl HasDependencies for LocalService2 {
        fn dependencies(&self) -> Dependencies {
            Dependencies::default()
        }
    }

    impl Resource for LocalService2 {
        type Provider = ();
        type Error = Error;
        type Output = RemoteService;

        async fn create(&self, (): &Self::Provider) -> Result<Self::Output, Self::Error> {
            let bucket_arn = *self.bucket_arn;
            Ok(RemoteService {
                service_id: format!("service-{}", bucket_arn.map(|c| c.to_string()).join("")),
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
            Ok(())
        }
    }

    async fn run_migration(store: &mut Store<()>) {
        log::warn!("running infra migration");

        let bucket_rez = store.destroy::<LocalBucket>("test-bucket").unwrap();

        let _service_a = store
            .resource(
                "test-service-a",
                LocalService2 {
                    bucket_arn: bucket_rez.migrate(|bucket| bucket.arn),
                },
            )
            .unwrap();

        let _service_b = store
            .resource(
                "test-service-b",
                LocalService2 {
                    bucket_arn: bucket_rez.migrate(|bucket| bucket.arn),
                },
            )
            .unwrap();
        write_graph_pdf(store, "destroy").await;
        log::info!("running plan: \n{}", store.get_schedule_string().unwrap());
        store.apply().await.unwrap();
    }
    run_migration(&mut store).await;
    backup("destroy").await;
    log::warn!("\n");
}
