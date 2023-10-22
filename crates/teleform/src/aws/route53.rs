//! Route53 infrastructure.
//! AWS certificate manager infrastructure.
use anyhow::Context;
use aws_config::SdkConfig;
use aws_sdk_route53::types::{
    Change, ChangeAction, ChangeBatch, ChangeStatus, ResourceRecord, ResourceRecordSet,
};

use crate::{self as tele, Local, TeleSync};

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
                            .action(ChangeAction::Create)
                            .resource_record_set({
                                let name = record.record_name.as_str();
                                let ttl = record.ttl.as_ref().clone();
                                let ty = record.type_is.as_str().into();
                                log::trace!("name: {name} ttl: {ttl:?} ty: {ty:?}");
                                ResourceRecordSet::builder()
                                    .name(name)
                                    .r#type(ty)
                                    .set_ttl(ttl)
                                    .set_resource_records(
                                        record.resource_records.as_ref().as_ref().map(
                                            |records: &Vec<String>| {
                                                records
                                                    .iter()
                                                    .map(|value| {
                                                        ResourceRecord::builder()
                                                            .value(value)
                                                            .build()
                                                    })
                                                    .collect::<Vec<_>>()
                                            },
                                        ),
                                    )
                                    .build()
                            })
                            .build(),
                    )
                    .build(),
            )
            .send()
            .await?;
        let mut info = out.change_info.context("missing change_info")?;
        log::info!("awaiting record change");
        let timeout_secs = 60;
        let start = std::time::Instant::now();
        while *info.status().context("missing change_info.status")? == ChangeStatus::Pending {
            if (std::time::Instant::now() - start).as_secs() >= timeout_secs {
                anyhow::bail!(
                    "finalization of record creation timed out after {timeout_secs} seconds"
                )
            }
            let out = client
                .get_change()
                .id(info.id.context("missing change_info.id")?)
                .send()
                .await?;
            info = out.change_info.context("missing change_info")?;
        }
        log::info!("...records in sync");
    }
    Ok(())
}

async fn update_record(
    _record: &mut Record,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
    _previous: &Record,
) -> anyhow::Result<()> {
    if apply {
        let _client = aws_sdk_route53::Client::new(cfg);
        todo!()
    }

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
