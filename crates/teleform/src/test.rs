use crate::{self as tele, *};

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
    bucket_arn: Remote<[u8; 8]>,
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
        let plan = store.plan().unwrap();
        store.apply(plan).await.unwrap();
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
        let service_a = store
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

        assert_eq!(
            service_a.action(),
            Action::Update,
            "`service_a` should update in response to `bucket_rez` updating"
        );

        write_graph_pdf(store, "update").await;
        log::info!("running plan: \n{}", store.get_schedule_string().unwrap());

        let legend = store.get_graph_legend().unwrap();
        println!("{}", store.get_schedule_string().unwrap());
        assert_eq!(
            2,
            legend.schedule.batches.len(),
            "update should be scheduled into 2 batches: \
            1 update in one batch and 2 loads in another"
        );
        let plan = store.plan().unwrap();
        store.apply(plan).await.unwrap();
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
        let plan = store.plan().unwrap();
        store.apply(plan).await.unwrap();
    }
    run_migration(&mut store).await;
    backup("destroy").await;
    log::warn!("\n");
}

/// Verify that resource types are automatically registered for orphan
/// auto-deletion when used via [`Store::resource`], without any explicit
/// [`Store::register`] call.
#[tokio::test]
async fn auto_register_orphan_delete() {
    let _ = env_logger::builder().try_init();

    let path =
        std::path::PathBuf::from(std::env!("CARGO_WORKSPACE_DIR")).join("test_output/auto_reg");
    if path.exists() {
        tokio::fs::remove_dir_all(&path).await.unwrap();
    }
    tokio::fs::create_dir_all(&path).await.unwrap();

    // Step 1: Create two buckets.
    let mut store = Store::new(&path, ());
    let _a = store
        .resource(
            "bucket-a",
            LocalBucket {
                name: "alpha".to_owned(),
            },
        )
        .unwrap();
    let _b = store
        .resource(
            "bucket-b",
            LocalBucket {
                name: "beta".to_owned(),
            },
        )
        .unwrap();
    let plan = store.plan().unwrap();
    assert!(plan.warnings.is_empty(), "no warnings on first apply");
    store.apply(plan).await.unwrap();

    // Both store files should exist.
    assert!(path.join("bucket-a.json").exists());
    assert!(path.join("bucket-b.json").exists());

    // Step 2: New store that only declares bucket-a.
    // bucket-b should be auto-detected as an orphan and scheduled for
    // deletion because LocalBucket was auto-registered via the
    // `store.resource()` call for bucket-a — no explicit `register()`.
    let mut store = Store::new(&path, ());
    let _a = store
        .resource(
            "bucket-a",
            LocalBucket {
                name: "alpha".to_owned(),
            },
        )
        .unwrap();
    let plan = store.plan().unwrap();
    assert!(
        plan.warnings.is_empty(),
        "no warnings expected: {:#?}",
        plan.warnings
    );
    let orphan = plan
        .actions
        .iter()
        .find(|a| a.id == "bucket-b")
        .expect("bucket-b should appear in the plan");
    assert_eq!(orphan.action, Action::Destroy);
    assert!(orphan.is_orphan);
    store.apply(plan).await.unwrap();

    // bucket-b store file should be gone.
    assert!(!path.join("bucket-b.json").exists());
    // bucket-a should still be there.
    assert!(path.join("bucket-a.json").exists());
}

/// Verify that orphaned resources of an unknown type (not used in the
/// current run and not manually registered) produce a warning suggesting
/// `store.register()`.
#[tokio::test]
async fn unknown_orphan_warning() {
    let _ = env_logger::builder().try_init();

    let path = std::path::PathBuf::from(std::env!("CARGO_WORKSPACE_DIR"))
        .join("test_output/unknown_orphan");
    if path.exists() {
        tokio::fs::remove_dir_all(&path).await.unwrap();
    }
    tokio::fs::create_dir_all(&path).await.unwrap();

    // Step 1: Create a bucket so there's a store file on disk.
    let mut store = Store::new(&path, ());
    let _bucket = store
        .resource(
            "my-bucket",
            LocalBucket {
                name: "lonely".to_owned(),
            },
        )
        .unwrap();
    let plan = store.plan().unwrap();
    store.apply(plan).await.unwrap();
    assert!(path.join("my-bucket.json").exists());

    // Step 2: New store that declares NO resources at all.
    // The bucket's type was never used in this run, so there's no deleter.
    // plan() should produce a warning.
    let mut store = Store::new(&path, ());
    let plan = store.plan().unwrap();
    assert_eq!(plan.warnings.len(), 1, "expected exactly one warning");
    assert!(
        plan.warnings[0].contains("my-bucket"),
        "warning should mention the orphan id"
    );
    assert!(
        plan.warnings[0].contains("register"),
        "warning should suggest register()"
    );

    // The store file should still exist (not auto-deleted).
    assert!(path.join("my-bucket.json").exists());
}

/// Verify that [`Store::clear_resources`] forgets declared resources but
/// preserves the type registry, enabling a "destroy everything" workflow.
#[tokio::test]
async fn clear_and_destroy_all() {
    let _ = env_logger::builder().try_init();

    let path = std::path::PathBuf::from(std::env!("CARGO_WORKSPACE_DIR"))
        .join("test_output/clear_destroy");
    if path.exists() {
        tokio::fs::remove_dir_all(&path).await.unwrap();
    }
    tokio::fs::create_dir_all(&path).await.unwrap();

    // Step 1: Create a bucket and a service.
    let mut store = Store::new(&path, ());
    let bucket = store
        .resource(
            "bucket",
            LocalBucket {
                name: "data".to_owned(),
            },
        )
        .unwrap();
    let _service = store
        .resource(
            "service",
            LocalService {
                bucket_arn: bucket.remote(|b| b.arn),
            },
        )
        .unwrap();
    let plan = store.plan().unwrap();
    store.apply(plan).await.unwrap();
    assert!(path.join("bucket.json").exists());
    assert!(path.join("service.json").exists());

    // Step 2: Same store instance — clear resources then plan.
    // Types are still registered from the resource() calls above, so
    // plan() should schedule both as orphan destroys with no warnings.
    store.clear_resources();
    let plan = store.plan().unwrap();
    assert!(
        plan.warnings.is_empty(),
        "no warnings: {:#?}",
        plan.warnings
    );
    assert_eq!(
        plan.actions.len(),
        2,
        "expected 2 destroy actions, got: {:#?}",
        plan.actions,
    );
    for action in &plan.actions {
        assert_eq!(action.action, Action::Destroy);
        assert!(action.is_orphan);
    }
    store.apply(plan).await.unwrap();

    // Both store files should be gone.
    assert!(!path.join("bucket.json").exists());
    assert!(!path.join("service.json").exists());
}
