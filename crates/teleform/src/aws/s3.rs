//! AWS S3 Bucket infrastructure.
use aws_config::SdkConfig;

use crate::{self as tele, Local, TeleSync};

#[derive(TeleSync, Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = SdkConfig)]
#[tele(create = create_bucket, update = update_bucket, delete = delete_bucket)]
pub struct Bucket {
    pub acl: Local<String>,
}

async fn create_bucket(
    bucket: &mut Bucket,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
) -> anyhow::Result<()> {
    if apply {
        let acl = aws_sdk_s3::types::BucketCannedAcl::from(bucket.acl.as_str());
        let client = aws_sdk_s3::Client::new(cfg);
        let _bucket = client.create_bucket().bucket(name).acl(acl).send().await?;
    }
    Ok(())
}

async fn update_bucket(
    bucket: &mut Bucket,
    apply: bool,
    cfg: &SdkConfig,
    _: &str,
    _: &Bucket,
) -> anyhow::Result<()> {
    if apply {
        let acl = aws_sdk_s3::types::BucketCannedAcl::from(bucket.acl.as_str());
        let client = aws_sdk_s3::Client::new(cfg);
        let _ = client.put_bucket_acl().acl(acl).send().await?;
    }

    Ok(())
}

async fn delete_bucket(_: &Bucket, apply: bool, cfg: &SdkConfig, name: &str) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_s3::Client::new(cfg);
        client.delete_bucket().bucket(name).send().await?;
    }
    Ok(())
}
