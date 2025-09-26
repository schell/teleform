//! Route53 infrastructure.
//! AWS certificate manager infrastructure.
use anyhow::Context;
use aws_config::SdkConfig;
use aws_sdk_route53::types::{
    self as aws, Change, ChangeAction, ChangeBatch, ChangeStatus, ResourceRecord, ResourceRecordSet,
};

use crate::{self as tele, Local, Remote, TeleEither, TeleSync};

// TODO: create a derive macro for TeleEither.

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AliasTarget {
    pub hosted_zone_id: Remote<String>,
    pub dns_name: Remote<String>,
    pub evaluate_target_health: Local<bool>,
}

impl TeleEither for AliasTarget {
    fn either(self, other: Self) -> Self {
        AliasTarget {
            hosted_zone_id: self.hosted_zone_id.either(other.hosted_zone_id),
            dns_name: self.dns_name.either(other.dns_name),
            evaluate_target_health: self
                .evaluate_target_health
                .either(other.evaluate_target_health),
        }
    }
}

impl TryFrom<AliasTarget> for aws::AliasTarget {
    type Error = aws_sdk_s3::error::BuildError;

    fn try_from(a: AliasTarget) -> Result<Self, Self::Error> {
        aws::AliasTarget::builder()
            .set_hosted_zone_id(a.hosted_zone_id.maybe_ref().cloned())
            .set_dns_name(a.dns_name.maybe_ref().cloned())
            .evaluate_target_health(*a.evaluate_target_health.as_ref())
            .build()
    }
}

#[derive(TeleSync, Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[tele(helper = SdkConfig)]
#[tele(create = create_record, update = update_record, delete = delete_record)]
pub struct Record {
    pub hosted_zone_id: Local<String>,
    pub record_name: Local<String>,
    #[serde(rename = "type")]
    pub type_is: Local<String>,
    pub ttl: Local<Option<i64>>,
    pub resource_records: Local<Option<Vec<String>>>,
    pub alias_target: Option<AliasTarget>,
}

async fn create_record(
    record: &mut Record,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_route53::Client::new(cfg);
        let out = client
            .change_resource_record_sets()
            .hosted_zone_id(record.hosted_zone_id.as_str())
            .change_batch(
                ChangeBatch::builder()
                    .changes(
                        Change::builder()
                            .action(ChangeAction::Upsert)
                            .resource_record_set({
                                let name = record.record_name.as_str();
                                let ttl = *record.ttl.as_ref();
                                let ty = record.type_is.as_str().into();
                                ResourceRecordSet::builder()
                                    .name(name)
                                    .r#type(ty)
                                    .set_ttl(ttl)
                                    .set_alias_target(
                                        if let Some(alias_target) = record.alias_target.clone() {
                                            Some(aws::AliasTarget::try_from(alias_target)?)
                                        } else {
                                            None
                                        },
                                    )
                                    .set_resource_records({
                                        if let Some(records) =
                                            record.resource_records.as_ref().as_ref()
                                        {
                                            let mut new_records = vec![];
                                            for record in records.iter() {
                                                new_records.push(
                                                    ResourceRecord::builder()
                                                        .value(record)
                                                        .build()?,
                                                );
                                            }
                                            Some(new_records)
                                        } else {
                                            None
                                        }
                                    })
                                    .build()?
                            })
                            .build()?,
                    )
                    .build()?,
            )
            .send()
            .await?;
        let mut info = out.change_info.context("missing change_info")?;
        log::info!("awaiting record change");
        let timeout_secs = 60;
        let start = std::time::Instant::now();
        while *info.status() == ChangeStatus::Pending {
            if (std::time::Instant::now() - start).as_secs() >= timeout_secs {
                anyhow::bail!(
                    "finalization of record creation timed out after {timeout_secs} seconds"
                )
            }
            let out = client.get_change().id(info.id).send().await?;
            info = out.change_info.context("missing change_info")?;
        }
        log::info!("...records in sync");
    }
    Ok(())
}

async fn update_record(
    record: &mut Record,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
    _previous: &Record,
) -> anyhow::Result<()> {
    create_record(record, apply, cfg, name).await?;
    Ok(())
}

async fn delete_record(
    _record: &Record,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        let _client = aws_sdk_route53::Client::new(cfg);
        todo!()
    }
    Ok(())
}
