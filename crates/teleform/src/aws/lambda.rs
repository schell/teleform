//! AWS Lambda infrastructure.
#![allow(clippy::unbuffered_bytes)]
use anyhow::Context;
use aws_config::SdkConfig;
use aws_sdk_lambda::types::{self as aws, Architecture, LastUpdateStatus};
use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    str::FromStr,
};

use crate::{self as tele, Local, Remote, TeleSync};

#[derive(TeleSync, Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = SdkConfig)]
#[tele(create = create_lambda, update = update_lambda, delete = delete_lambda)]
pub struct Lambda {
    #[tele(should_recreate)]
    pub name: Local<String>,
    // ARN of the role to use for this lambda.
    pub role_arn: Remote<String>,
    pub handler: Local<String>,
    pub zip_file_path: Local<String>,
    #[serde(default)]
    pub zip_file_hash: Remote<String>,
    pub architecture: Local<Option<String>>,
    #[serde(default)]
    pub environment: Local<Option<BTreeMap<String, String>>>,
    #[serde(default)]
    // Time (in seconds) the function can run before being terminated.
    pub timeout: Local<Option<i32>>,
    // Known after creation.
    pub arn: Remote<String>,
    // Known after creation/update.
    pub version: Remote<String>,
}

impl Lambda {
    pub fn environment(&self) -> Option<aws::Environment> {
        let env = self.environment.as_ref().as_ref()?;
        Some(
            aws::builders::EnvironmentBuilder::default()
                .set_variables(Some(HashMap::from_iter(
                    env.iter().map(|(k, v)| (k.to_string(), v.to_string())),
                )))
                .build(),
        )
    }
}

async fn create_lambda(
    lambda: &mut Lambda,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_lambda::Client::new(cfg);
        let file = std::fs::File::open(lambda.zip_file_path.as_ref())?;
        let bytes: Vec<u8> = file.bytes().try_fold(vec![], |mut acc, byte| {
            acc.push(byte?);
            anyhow::Ok(acc)
        })?;
        let arch = lambda
            .architecture
            .as_ref()
            .as_ref()
            .and_then(|s| Architecture::from_str(s).ok())
            .unwrap_or(Architecture::Arm64);
        let blob = aws_sdk_lambda::primitives::Blob::new(bytes);
        let out = client
            .create_function()
            .function_name(lambda.name.as_ref())
            .package_type(aws_sdk_lambda::types::PackageType::Zip)
            .runtime(aws_sdk_lambda::types::Runtime::Providedal2)
            .set_architectures(Some(vec![arch]))
            .set_environment(lambda.environment())
            .set_timeout(*lambda.timeout.as_ref())
            .role(
                lambda
                    .role_arn
                    .maybe_ref()
                    .context("unknown lambda role arn")?,
            )
            .handler(&lambda.handler.0)
            .publish(true)
            .code(
                aws_sdk_lambda::types::builders::FunctionCodeBuilder::default()
                    .zip_file(blob)
                    .build(),
            )
            .send()
            .await?;
        lambda.arn = out.function_arn.context("missing arn")?.into();
        lambda.version = out.version.context("missing version")?.into();
        log::info!("...created lambda {name}");
    }
    Ok(())
}

async fn update_lambda(
    lambda: &mut Lambda,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
    previous: &Lambda,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_lambda::Client::new(cfg);

        async fn await_finalization(
            client: &aws_sdk_lambda::Client,
            lambda: &Lambda,
        ) -> anyhow::Result<()> {
            // timeout after 5 minutes
            let timeout_secs = 60 * 5;
            let start = std::time::Instant::now();
            log::info!("awaiting update finialization");
            loop {
                let out = client
                    .get_function_configuration()
                    .function_name(lambda.name.as_str())
                    .send()
                    .await?;
                let last_update_status = out.last_update_status.context("missing status")?;
                if last_update_status == LastUpdateStatus::Successful {
                    break;
                }
                if (std::time::Instant::now() - start).as_secs() >= timeout_secs {
                    anyhow::bail!("finalization timed out after {timeout_secs} seconds");
                }
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
            Ok(())
        }

        let mut needs_new_version = false;
        if lambda.zip_file_hash != previous.zip_file_hash {
            log::debug!("updating lambda code");
            needs_new_version = true;
            let arch = lambda
                .architecture
                .as_ref()
                .as_ref()
                .and_then(|s| Architecture::from_str(s).ok())
                .unwrap_or(Architecture::Arm64);
            let out = client
                .update_function_code()
                .function_name(lambda.name.as_ref())
                .set_architectures(Some(vec![arch]))
                .zip_file({
                    let file = std::fs::File::open(lambda.zip_file_path.as_ref())?;
                    let bytes: Vec<u8> = file.bytes().try_fold(vec![], |mut acc, byte| {
                        acc.push(byte?);
                        anyhow::Ok(acc)
                    })?;
                    log::debug!("sending {} bytes of code/program", bytes.len());
                    aws_sdk_lambda::primitives::Blob::new(bytes)
                })
                .send()
                .await?;

            let last_update_status = out.last_update_status.context("missing status")?;
            let last_update_status_reason = out
                .last_update_status_reason
                .unwrap_or_else(|| "unknown".to_string());
            log::debug!("last_update_status: {last_update_status:?}");
            log::debug!("last_update_status_reason: {last_update_status_reason}");
            anyhow::ensure!(
                last_update_status != LastUpdateStatus::Failed,
                "update failed!"
            );
            await_finalization(&client, lambda).await?;
            log::info!("...done")
        }

        if lambda.environment != previous.environment
            || lambda.role_arn != previous.role_arn
            || lambda.handler != previous.handler
            || lambda.timeout != previous.timeout
        {
            log::info!("updating lambda configuration");
            needs_new_version = true;
            let out = client
                .update_function_configuration()
                .function_name(format!("{}:$LATEST", lambda.name.as_ref()))
                .set_environment(lambda.environment())
                .set_timeout(*lambda.timeout.as_ref())
                .role(
                    lambda
                        .role_arn
                        .maybe_ref()
                        .context("unknown lambda role arn")?,
                )
                .handler(lambda.handler.as_ref())
                .runtime(aws_sdk_lambda::types::Runtime::Providedal2)
                .send()
                .await?;
            let last_update_status = out.last_update_status.context("missing status")?;
            let last_update_status_reason = out
                .last_update_status_reason
                .unwrap_or_else(|| "unknown".to_string());
            log::debug!("last_update_status: {last_update_status:?}");
            log::debug!("last_update_status_reason: {last_update_status_reason}");
            anyhow::ensure!(
                last_update_status != LastUpdateStatus::Failed,
                "update failed!"
            );
            await_finalization(&client, lambda).await?;
            log::info!("...done");
        }

        if needs_new_version {
            log::debug!("publishing a new lambda version");
            let out = client
                .publish_version()
                .function_name(lambda.name.as_ref())
                .send()
                .await?;
            lambda.version = out.version.context("missing version")?.into();
            log::info!("...pushed a new version");
        }
        log::info!("...updated lambda {name}");
    }

    Ok(())
}

async fn delete_lambda(
    lambda: &Lambda,
    apply: bool,
    cfg: &SdkConfig,
    _: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_lambda::Client::new(cfg);
        let _ = client
            .delete_function()
            .function_name(lambda.name.as_ref())
            .send()
            .await?;
    }
    Ok(())
}

#[derive(TeleSync, Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = SdkConfig)]
#[tele(create = create_added_perm, update = update_added_perm, delete = delete_added_perm)]
pub struct LambdaAddedPermission {
    // The function ARN.
    // This should _probably_ include the version.
    // `Remote` because it is usually dependent on upstream `Remote` values.
    pub function_arn: Remote<String>,
    // A unique statement id that differentiates it from others in the same policy.
    // Eg "apigateway-test-2"
    pub statement_id: Option<Local<String>>,
    // The action that the principal can use on the function.
    // Eg "lambda:InvokeFunction" or "lambda:GetFunction"
    pub action: Local<String>,
    // The Amazon Web Service or Amazon Web Services account that invokes the function.
    pub principal: Local<String>,
    // For Amazon Web Services, the ARN of the Amazon Web Services resource that
    // invokes the function.
    // `Remote` because it is usually dependent on upstream `Remote` values.
    pub source_arn: Remote<String>,
}

async fn create_added_perm(
    perm: &mut LambdaAddedPermission,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_lambda::Client::new(cfg);
        let out =
            client
                .add_permission()
                .function_name(
                    perm.function_arn
                        .maybe_ref()
                        .context("cannot add permission - missing function arn")?,
                )
                .statement_id(
                    perm.statement_id
                        .as_deref()
                        .map(String::as_str)
                        .unwrap_or(name),
                )
                .action(perm.action.as_str())
                .principal(perm.principal.as_str())
                .source_arn(perm.source_arn.maybe_ref().with_context(|| {
                    format!("cannot add permission {name} - missing source_arn")
                })?)
                .send()
                .await?;
        log::info!("added permission:\n{:#?}", out.statement());
    }
    Ok(())
}

async fn update_added_perm(
    perm: &mut LambdaAddedPermission,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
    previous: &LambdaAddedPermission,
) -> anyhow::Result<()> {
    if apply {
        delete_added_perm(previous, apply, cfg, name).await?;
        create_added_perm(perm, apply, cfg, name).await?;
    }

    Ok(())
}

async fn delete_added_perm(
    perm: &LambdaAddedPermission,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_lambda::Client::new(cfg);
        let _out = client
            .remove_permission()
            .function_name(
                perm.function_arn
                    .maybe_ref()
                    .context("cannot remove permission - missing function arn")?,
            )
            .statement_id(
                perm.statement_id
                    .as_deref()
                    .map(String::as_str)
                    .unwrap_or(name),
            )
            .send()
            .await?;
    }
    Ok(())
}
