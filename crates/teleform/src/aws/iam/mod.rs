//! IAM infrastructure.
use anyhow::Context;
use aws_config::SdkConfig;

use crate::{self as tele, Local, Remote, TeleSync};

/// AWS policy resource
#[derive(TeleSync, Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[tele(helper = SdkConfig)]
#[tele(create = create_policy, update = update_policy, delete = delete_policy)]
pub struct Policy {
    pub document: Local<serde_json::Value>,
    pub arn: Remote<String>,
    pub version_id: Remote<Option<String>>,
}

async fn create_policy(
    policy: &mut Policy,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_iam::Client::new(cfg);
        let iam_policy = client
            .create_policy()
            .policy_name(name)
            .policy_document(&serde_json::to_string(&policy.document)?)
            .send()
            .await?
            .policy
            .context("no policy")?;
        policy.arn = iam_policy
            .arn
            .context("missing arn from policy creation")?
            .into();
        log::info!("...created policy {name}");
    }
    Ok(())
}

async fn update_policy(
    policy: &mut Policy,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
    _: &Policy,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_iam::Client::new(cfg);
        let out = client
            .create_policy_version()
            .policy_arn(
                policy
                    .arn
                    .maybe_ref()
                    .context("cannot update policy - missing arn")?,
            )
            .policy_document(&serde_json::to_string(&policy.document)?)
            .set_as_default(true)
            .send()
            .await?;
        policy.version_id = out
            .policy_version
            .and_then(|pv| pv.version_id().map(|s| s.to_string()))
            .into();
    }

    Ok(())
}

async fn delete_policy(
    policy: &Policy,
    apply: bool,
    cfg: &SdkConfig,
    _: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_iam::Client::new(cfg);
        let _ = client
            .delete_policy()
            .policy_arn(policy.arn.maybe_ref().context("missing arn")?)
            .send()
            .await?;
    }
    Ok(())
}

/// AWS role resources.
#[derive(TeleSync, Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = aws_config::SdkConfig)]
#[tele(create = create_role, update = update_role, delete = delete_role)]
pub struct Role {
    #[tele(should_recreate)]
    pub document: Local<serde_json::Value>,
    pub attached_policy_arn: Local<Option<Remote<String>>>,
    pub arn: Remote<String>,
}

async fn create_role(
    role: &mut Role,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_iam::Client::new(cfg);
        let iam_role = client
            .create_role()
            .role_name(name)
            .assume_role_policy_document(serde_json::to_string(&role.document)?)
            .send()
            .await?
            .role
            .context("no role")?;
        log::info!("...created role {name}");

        if let Some(policy) = role.attached_policy_arn.as_ref() {
            attach_policy(cfg, name, policy.maybe_ref().context("missing policy arn")?).await?;
        }
        role.arn = iam_role.arn.into();
    }
    Ok(())
}

async fn update_role(
    role: &mut Role,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
    previous: &Role,
) -> anyhow::Result<()> {
    if role.attached_policy_arn != previous.attached_policy_arn {
        if let Some(prev_policy) = previous.attached_policy_arn.as_ref() {
            log::info!("detaching previous policy {prev_policy} from role {name}");
            if apply {
                detach_policy(
                    cfg,
                    name,
                    prev_policy
                        .maybe_ref()
                        .context("cannot detach - unknown policy arn")?,
                )
                .await?;
            }
        }
        if let Some(new_policy) = role.attached_policy_arn.as_ref() {
            log::info!("attaching new policy {new_policy} to role {name}");
            if apply {
                attach_policy(
                    cfg,
                    name,
                    new_policy
                        .maybe_ref()
                        .context("cannot attach policy - unknown arn")?,
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn delete_role(role: &Role, apply: bool, cfg: &SdkConfig, name: &str) -> anyhow::Result<()> {
    if apply {
        if let Some(policy_arn) = role.attached_policy_arn.as_ref() {
            detach_policy(
                cfg,
                name,
                policy_arn
                    .maybe_ref()
                    .context("cannot detach policy - unknown arn")?,
            )
            .await?;
        }
        let client = aws_sdk_iam::Client::new(cfg);
        let _ = client.delete_role().role_name(name).send().await?;
    }
    Ok(())
}

//pub async fn create_role(
//    cfg: &SdkConfig,
//    name: &str,
//    document: &serde_json::Value,
//) -> anyhow::Result<aws_sdk_iam::types::Role> {
//    let client = aws_sdk_iam::Client::new(cfg);
//    let role = client
//        .create_role()
//        .role_name(name)
//        .assume_role_policy_document(serde_json::to_string(document)?)
//        .send()
//        .await?
//        .role
//        .context("no role")?;
//    log::info!("...created role {name}");
//    Ok(role)
//}

//pub async fn delete_role(cfg: &SdkConfig, name: &str) -> anyhow::Result<()> {
//    let client = aws_sdk_iam::Client::new(cfg);
//    let _ = client.delete_role().role_name(name).send().await?;
//    Ok(())
//}

pub async fn attach_policy(
    cfg: &SdkConfig,
    role_name: &str,
    policy_arn: &str,
) -> anyhow::Result<()> {
    let client = aws_sdk_iam::Client::new(cfg);
    let _ = client
        .attach_role_policy()
        .role_name(role_name)
        .policy_arn(policy_arn)
        .send()
        .await?;
    log::info!("...attached policy {policy_arn} to role {role_name}");
    Ok(())
}

pub async fn detach_policy(
    cfg: &SdkConfig,
    role_name: &str,
    policy_arn: &str,
) -> anyhow::Result<()> {
    let client = aws_sdk_iam::Client::new(cfg);
    let _ = client
        .detach_role_policy()
        .role_name(role_name)
        .policy_arn(policy_arn)
        .send()
        .await?;
    Ok(())
}
