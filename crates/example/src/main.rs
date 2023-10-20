//! Example of using teleform in a Rust command-line program.
use anyhow::Context;
use aws_config::SdkConfig;
use clap::Parser;
use tele::{aws, Local, Remote, Store};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Infra {
    pub lambda_policy: aws::iam::Policy,
    pub lambda_role: aws::iam::Role,
    pub lambda: aws::lambda::Lambda,
    pub dydb_table: aws::dynamodb::Table,
    pub apigateway: aws::apigatewayv2::ApiGatewayV2,
    pub stage: aws::apigatewayv2::Stage,
    pub apigateway_lambda_perm: aws::lambda::LambdaAddedPermission,
    pub integration: aws::apigatewayv2::Integration,
    pub catchall_route: aws::apigatewayv2::Route,
}

/// Apply the infrastructure to our store, synchronizing it with our remote resources.
pub async fn infrastructure<'b, 'a: 'b>(
    account_id: String,
    store: &'b mut Store<&'a SdkConfig>,
) -> anyhow::Result<Infra> {
    let dydb_table = store
        .sync(
            "crud-table",
            aws::dynamodb::Table {
                table_name: "crud-table".into(),
                key_schema: vec![aws::dynamodb::KeySchemaElement::partition_key(
                    "id",
                    aws::dynamodb::AttributeType::String,
                )]
                .into(),
                billing_mode: aws::dynamodb::BillingMode::PayPerRequest.into(),
                ..Default::default()
            },
        )
        .await?;

    // TODO: consider making `finalize` part of TeleSync.
    if store.apply {
        aws::dynamodb::finalize(&dydb_table, store.cfg).await?;
        log::info!("...done");
    }

    let lambda_policy = store
        .sync(
            "lambda-apigateway-policy",
            aws::iam::Policy {
                document: serde_json::json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Sid": "Stmt1428341300017",
                            "Action": [
                                "dynamodb:DeleteItem",
                                "dynamodb:GetItem",
                                "dynamodb:PutItem",
                                "dynamodb:Query",
                                "dynamodb:Scan",
                                "dynamodb:UpdateItem"
                            ],
                            "Effect": "Allow",
                            "Resource": "*"
                        },
                        {
                            "Sid": "",
                            "Resource": "*",
                            "Action": [
                                "logs:CreateLogGroup",
                                "logs:CreateLogStream",
                                "logs:PutLogEvents"
                            ],
                            "Effect": "Allow"
                        }
                    ]
                })
                .into(),
                ..Default::default()
            },
        )
        .await?;

    let lambda_role = store
        .sync(
            "lambda-apigateway-role",
            aws::iam::Role {
                document: serde_json::json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Action": [
                                "sts:AssumeRole"
                            ],
                            "Principal": {
                                "Service": [
                                    "lambda.amazonaws.com"
                                ]
                            }
                        }
                    ]
                })
                .into(),
                attached_policy_arn: Some(lambda_policy.arn.clone()).into(),
                ..Default::default()
            },
        )
        .await?;

    let zip_file_path: Local<String> = "target/lambda/example-lambda/bootstrap.zip".into();
    let lambda = store
        .sync(
            "lambda-function",
            aws::lambda::Lambda {
                name: "teleform-example-lambda".into(),
                role_arn: lambda_role.arn.clone().into(),
                handler: "bootstrap".into(),
                zip_file_hash: aws::lambda::sha256_digest(zip_file_path.as_ref())?
                    .map(Remote::Remote)
                    .unwrap_or(Remote::Unknown),
                zip_file_path,
                architecture: Some("arm64".into()).into(),
                ..Default::default()
            },
        )
        .await?;

    let apigateway = store
        .sync(
            "gateway",
            aws::apigatewayv2::ApiGatewayV2 {
                ..Default::default()
            },
        )
        .await?;

    let stage = store
        .sync(
            "stage",
            aws::apigatewayv2::Stage {
                api_id: apigateway.api_id.clone(),
                stage_name: "$default".into(),
                auto_deploy: true.into(),
            }
        )
        .await?;

    // Add permission for the http gateway to call the lambda
    let region = store.cfg.region().context("unknown region")?;
    let source_arn = apigateway
        .api_id
        .maybe_ref()
        .map(|api_id| format!("arn:aws:execute-api:{region}:{account_id}:{api_id}/*/*/*",).into())
        .unwrap_or_default();
    let apigateway_lambda_perm = store
        .sync(
            "apigateway-lambda-invoke-perm",
            aws::lambda::LambdaAddedPermission {
                function_arn: (|| -> Option<Remote<String>> {
                    let arn = lambda.arn.maybe_ref()?;
                    let version = lambda.version.maybe_ref()?;
                    Some(format!("{arn}:{version}").into())
                })()
                .unwrap_or_default(),
                action: "lambda:InvokeFunction".into(),
                principal: "apigateway.amazonaws.com".into(),
                source_arn,
                ..Default::default()
            },
        )
        .await?;

    let integration = store
        .sync(
            "integration",
            aws::apigatewayv2::Integration {
                api_id: apigateway.api_id.clone().into(),
                integration_uri: (|| -> Option<Remote<String>> {
                    let arn = lambda.arn.maybe_ref()?;
                    let version = lambda.version.maybe_ref()?;
                    Some(format!("{arn}:{version}").into())
                })()
                .unwrap_or_default(),
                ..Default::default()
            },
        )
        .await?;

    let catchall_route = store
        .sync(
            "catchall-route",
            aws::apigatewayv2::Route {
                api_id: apigateway.api_id.clone(),
                route_key: "ANY /{proxy+}".into(),
                target: integration
                    .integration_id
                    .maybe_ref()
                    .cloned()
                    .map(|id| format!("integrations/{id}"))
                    .into(),
                ..Default::default()
            },
        )
        .await?;

    Ok(Infra {
        lambda_policy,
        lambda_role,
        lambda,
        dydb_table,
        apigateway,
        stage,
        integration,
        catchall_route,
        apigateway_lambda_perm,
    })
}

#[derive(Parser)]
#[clap(author, version, about)]
struct Cli {
    /// Sets the verbosity level
    #[clap(short, action = clap::ArgAction::Count)]
    verbosity: u8,
    /// Whether to apply any changes. Required to change infrastructure.
    #[clap(long)]
    apply: bool,

    /// Delete the infrastructure (requires interactive confirmation).
    /// Must be paired with --apply to patch infrastructure,
    /// otherwise the changes will only be printed.
    #[clap(long)]
    delete: bool,

    /// Your AWS account id.
    #[clap(long)]
    account_id: String,
}

#[::tokio::main]
async fn main() -> anyhow::Result<()> {
    let Cli {
        verbosity,
        apply,
        delete,
        account_id,
    } = Cli::parse();

    let level = match verbosity {
        0 => log::LevelFilter::Warn,
        1 => log::LevelFilter::Info,
        2 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    };
    env_logger::Builder::default()
        .filter_level(log::LevelFilter::Warn)
        .filter_module("tele", level)
        .filter_module("example", level)
        .init();

    log::info!("apply: {apply:?}");
    log::info!("delete: {delete}");

    let workspace_dir = tele::cli::find_workspace_dir()?;
    let store_path = workspace_dir.join("default_store.json");
    let backup_store_path = store_path.with_extension("bak.json");
    log::debug!("using store file: {}", store_path.display());

    let sdk_cfg = aws_config::from_env().load().await;
    let mut store = tele::cli::create_store(&store_path, &backup_store_path, &sdk_cfg, apply)?;

    let maybe_infra = if delete {
        log::warn!("deleting previous infrastructure!");
        None
    } else {
        let infra = infrastructure(account_id, &mut store).await?;
        log::info!(
            "Play with your app at {}",
            infra
                .apigateway
                .api_endpoint
                .maybe_ref()
                .context("missing api_endpoint")?
        );
        Some(infra)
    };

    let has_prunes = tele::cli::display_prunes(&store);
    if has_prunes {
        let perform_prune = if delete && apply {
            tele::cli::confirm_delete("you're the man now, dog")?
        } else {
            // don't worry, if `apply` is `false` the resources still won't be pruned
            true
        };
        if perform_prune {
            tele::aws::prune(&mut store).await?;
        }
    }

    if apply {
        // ensure that pruning deleted all unused resources (or bail)
        let remaining_prunes = store.get_prunes();
        anyhow::ensure!(
            remaining_prunes.is_empty(),
            "unhandled prunes {remaining_prunes:#?}"
        );

        if let Some(infra) = maybe_infra {
            // Write the final infrastructure definition so we can use it
            // in our app.
            std::fs::write(
                store_path.parent().unwrap().join("infrastructure.json"),
                // UNWRAP: safe because infra can always be serialized (or we want to panic)
                serde_json::to_string_pretty(&infra).unwrap(),
            )?;
        }
        // Remove the backup
        std::fs::remove_file(backup_store_path)?;
    }

    Ok(())
}
