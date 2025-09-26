//! AWS DynamoDB infrastructure.
use crate::{self as tele, Local, Remote, TeleSync};
use anyhow::Context;
use aws_config::SdkConfig;
use aws_sdk_dynamodb::types as aws;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum KeyType {
    Hash,
    Range,
}

impl From<KeyType> for aws::KeyType {
    fn from(value: KeyType) -> Self {
        match value {
            KeyType::Hash => aws::KeyType::Hash,
            KeyType::Range => aws::KeyType::Range,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AttributeType {
    Binary,
    Number,
    String,
}

impl From<AttributeType> for aws::ScalarAttributeType {
    fn from(value: AttributeType) -> Self {
        match value {
            AttributeType::Binary => aws::ScalarAttributeType::B,
            AttributeType::Number => aws::ScalarAttributeType::N,
            AttributeType::String => aws::ScalarAttributeType::S,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KeySchemaElement {
    pub attribute_name: String,
    pub key_type: KeyType,
    pub attribute_type: AttributeType,
}

impl TryFrom<&KeySchemaElement> for aws::KeySchemaElement {
    type Error = aws_sdk_s3::error::BuildError;

    fn try_from(value: &KeySchemaElement) -> Result<Self, Self::Error> {
        aws::KeySchemaElement::builder()
            .attribute_name(value.attribute_name.clone())
            .key_type(value.key_type.into())
            .build()
    }
}

impl TryFrom<&KeySchemaElement> for aws::AttributeDefinition {
    type Error = aws_sdk_s3::error::BuildError;

    fn try_from(value: &KeySchemaElement) -> Result<Self, Self::Error> {
        aws::AttributeDefinition::builder()
            .attribute_name(value.attribute_name.clone())
            .attribute_type(value.attribute_type.into())
            .build()
    }
}

impl KeySchemaElement {
    pub fn partition_key(name: impl Into<String>, type_is: AttributeType) -> Self {
        KeySchemaElement {
            attribute_name: name.into(),
            key_type: KeyType::Hash,
            attribute_type: type_is,
        }
    }

    pub fn sort_key(name: impl Into<String>, type_is: AttributeType) -> Self {
        KeySchemaElement {
            attribute_name: name.into(),
            key_type: KeyType::Range,
            attribute_type: type_is,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TableClass {
    #[default]
    Standard,
    StandardInfrequentAccess,
}

impl From<TableClass> for aws::TableClass {
    fn from(value: TableClass) -> Self {
        match value {
            TableClass::Standard => aws::TableClass::Standard,
            TableClass::StandardInfrequentAccess => aws::TableClass::StandardInfrequentAccess,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BillingMode {
    PayPerRequest,
    Provisioned {
        read_capacity_units: i64,
        write_capacity_units: i64,
    },
}

impl Default for BillingMode {
    fn default() -> Self {
        BillingMode::Provisioned {
            read_capacity_units: 5,
            write_capacity_units: 5,
        }
    }
}

impl From<BillingMode> for aws::BillingMode {
    fn from(value: BillingMode) -> Self {
        match value {
            BillingMode::PayPerRequest => aws::BillingMode::PayPerRequest,
            BillingMode::Provisioned { .. } => aws::BillingMode::Provisioned,
        }
    }
}

impl TryFrom<BillingMode> for Option<aws::ProvisionedThroughput> {
    type Error = aws_sdk_s3::error::BuildError;

    fn try_from(value: BillingMode) -> Result<Self, Self::Error> {
        match value {
            BillingMode::PayPerRequest => Ok(None),
            BillingMode::Provisioned {
                read_capacity_units,
                write_capacity_units,
            } => Ok(Some(
                aws::ProvisionedThroughput::builder()
                    .read_capacity_units(read_capacity_units)
                    .write_capacity_units(write_capacity_units)
                    .build()?,
            )),
        }
    }
}

#[derive(TeleSync, Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = SdkConfig)]
#[tele(
    create = create_table,
    create_finalize = create_finalize_table,
    update = update_table,
    delete = delete_table
)]
pub struct Table {
    pub table_name: Local<String>,
    pub table_class: Local<TableClass>,
    #[tele(should_recreate)]
    pub key_schema: Local<Vec<KeySchemaElement>>,
    pub billing_mode: Local<BillingMode>,
    // Known after creation.
    pub arn: Remote<String>,
    // Known after creation.
    pub id: Remote<String>,
}

async fn create_table(
    table: &mut Table,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_dynamodb::Client::new(cfg);
        let out = client
            .create_table()
            .table_name(table.table_name.as_str())
            .table_class(table.table_class.0.into())
            .billing_mode(table.billing_mode.0.into())
            .set_provisioned_throughput(table.billing_mode.0.try_into()?)
            .set_key_schema(if table.key_schema.is_empty() {
                None
            } else {
                Some({
                    let mut ks = vec![];
                    for k in table.key_schema.iter() {
                        ks.push(k.try_into()?);
                    }
                    ks
                })
            })
            .set_attribute_definitions(if table.key_schema.is_empty() {
                None
            } else {
                Some({
                    let mut ks = vec![];
                    for k in table.key_schema.iter() {
                        ks.push(k.try_into()?);
                    }
                    ks
                })
            })
            .send()
            .await?;
        let description = out.table_description.context("missing table description")?;
        table.arn = description.table_arn.context("table missing arn")?.into();
        if let Some(id) = description.table_id {
            table.id = id.into();
        }
    }
    Ok(())
}

pub async fn create_finalize_table(
    table: &mut Table,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        // timeout after 5 minutes
        let timeout_secs = 60 * 5;
        let start = std::time::Instant::now();
        log::info!("awaiting table creation finialization");
        loop {
            let client = aws_sdk_dynamodb::Client::new(cfg);
            let out = client
                .describe_table()
                .table_name(&table.table_name.0)
                .send()
                .await?;
            let table_info = out.table.context("missing table description")?;
            if table_info.table_status == Some(aws::TableStatus::Active) {
                log::info!("...finalized");
                return Ok(());
            }
            anyhow::ensure!(
                table_info.table_status == Some(aws::TableStatus::Creating),
                "table finalization failed, table status: {:?}",
                table_info.table_status
            );
            if (std::time::Instant::now() - start).as_secs() >= timeout_secs {
                anyhow::bail!("finalization timed out after {timeout_secs} seconds");
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    } else {
        Ok(())
    }
}

async fn update_table(
    _table: &mut Table,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
    _previous: &Table,
) -> anyhow::Result<()> {
    if apply {
        let _client = aws_sdk_dynamodb::Client::new(cfg);
        todo!()
    }

    Ok(())
}

async fn delete_table(
    table: &Table,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_dynamodb::Client::new(cfg);
        let _ = client
            .delete_table()
            .table_name(table.table_name.as_ref())
            .send()
            .await?;
    }
    Ok(())
}
