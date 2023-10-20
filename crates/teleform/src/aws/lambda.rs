//! AWS Lambda infrastructure.
use anyhow::Context;
use aws_config::SdkConfig;
use aws_sdk_lambda::types::{Architecture, LastUpdateStatus};
use std::{io::Read, str::FromStr};

use crate::{Local, Remote, TeleSync, self as tele};

/// Returns the sha256 digest of the file at the given path *if it exists*.
/// If the file does _not_ exist it returns `Ok(None)`.
pub fn sha256_digest(path: impl AsRef<std::path::Path>) -> anyhow::Result<Option<String>> {
    log::debug!("determining sha256 of {}", path.as_ref().display());
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

#[derive(TeleSync, Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = &'a SdkConfig)]
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
    // Known after creation.
    pub arn: Remote<String>,
    // Known after creation/update.
    pub version: Remote<String>,
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
        let bytes: Vec<u8> = file.bytes().into_iter().try_fold(vec![], |mut acc, byte| {
            acc.push(byte?);
            anyhow::Ok(acc)
        })?;
        let arch = lambda
            .architecture
            .as_ref()
            .as_ref()
            .map(|s| Architecture::from_str(&s).ok())
            .flatten()
            .unwrap_or(Architecture::Arm64);
        let blob = aws_sdk_lambda::primitives::Blob::new(bytes);
        let out = client
            .create_function()
            .function_name(lambda.name.as_ref())
            .package_type(aws_sdk_lambda::types::PackageType::Zip)
            .runtime(aws_sdk_lambda::types::Runtime::Providedal2)
            .set_architectures(Some(vec![arch]))
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
        if lambda.zip_file_hash != previous.zip_file_hash {
            log::debug!("updating lambda code");
            let arch = lambda
                .architecture
                .as_ref()
                .as_ref()
                .map(|s| Architecture::from_str(&s).ok())
                .flatten()
                .unwrap_or(Architecture::Arm64);
            let out = client
                .update_function_code()
                .function_name(lambda.name.as_ref())
                .set_architectures(Some(vec![arch]))
                .zip_file({
                    let file = std::fs::File::open(lambda.zip_file_path.as_ref())?;
                    let bytes: Vec<u8> =
                        file.bytes().into_iter().try_fold(vec![], |mut acc, byte| {
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
            anyhow::ensure!(last_update_status != LastUpdateStatus::Failed, "update failed!");
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

            log::debug!("publishing a new lambda version");
            let out = client
                .publish_version()
                .function_name(lambda.name.as_ref())
                .send()
                .await?;
            lambda.version = out.version.context("missing version")?.into();

            log::info!("...done")
        }
        //let _ = client
        //    .update_function_configuration()
        //    .function_name(lambda.name.as_ref())
        //    .role(
        //        lambda
        //            .role_arn
        //            .maybe_ref()
        //            .context("unknown lambda role arn")?,
        //    )
        //    .handler(lambda.handler.as_ref())
        //    .runtime(aws_sdk_lambda::types::Runtime::Providedal2)
        //    .send()
        //    .await?;
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
#[tele(helper = &'a SdkConfig)]
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
