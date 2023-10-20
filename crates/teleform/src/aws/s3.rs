//! AWS S3 Bucket infrastructure.
use anyhow::Context;
use aws_config::SdkConfig;
use aws_sdk_lambda::primitives::ByteStream;

use crate::{self as tele, Local, TeleSync};

#[derive(TeleSync, Debug, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = SdkConfig)]
#[tele(create = create_bucket, update = update_bucket, delete = delete_bucket)]
pub struct Bucket {
    pub acl: Local<String>,
    pub bucket_name: Local<String>,
}

async fn create_bucket(
    bucket: &mut Bucket,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
) -> anyhow::Result<()> {
    if bucket.bucket_name.is_empty() {
        log::warn!("bucket was created without a name - using the resource name");
        bucket.bucket_name = name.to_string().into();
    }
    if apply {
        let acl = aws_sdk_s3::types::BucketCannedAcl::from(bucket.acl.as_str());
        let client = aws_sdk_s3::Client::new(cfg);
        let _bucket = client
            .create_bucket()
            .bucket(bucket.bucket_name.as_str())
            .acl(acl)
            .send()
            .await?;
    }
    Ok(())
}

async fn update_bucket(
    bucket: &mut Bucket,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
    _: &Bucket,
) -> anyhow::Result<()> {
    if bucket.bucket_name.is_empty() {
        log::warn!("bucket was created without a name - using the resource name");
        bucket.bucket_name = name.to_string().into();
    }
    if apply {
        let acl = aws_sdk_s3::types::BucketCannedAcl::from(bucket.acl.as_str());
        let client = aws_sdk_s3::Client::new(cfg);
        let _ = client.put_bucket_acl().acl(acl).send().await?;
    }

    Ok(())
}

async fn delete_bucket(
    bucket: &Bucket,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
) -> anyhow::Result<()> {
    let bucket_name = if bucket.bucket_name.is_empty() {
        name
    } else {
        bucket.bucket_name.as_str()
    };
    if apply {
        let client = aws_sdk_s3::Client::new(cfg);
        client.delete_bucket().bucket(bucket_name).send().await?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ObjectFile {
    pub path: std::path::PathBuf,
    pub hash: String,
}

#[derive(TeleSync, Debug, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = SdkConfig)]
#[tele(create = create_object, update = update_object, delete = delete_object)]
pub struct Object {
    #[tele(should_recreate)]
    pub acl: Local<String>,
    #[tele(should_recreate)]
    pub key: Local<String>,
    #[tele(should_recreate)]
    pub bucket: Local<String>,
    #[tele(should_recreate)]
    pub body: Local<ObjectFile>,
}

async fn create_object(
    object: &mut Object,
    apply: bool,
    cfg: &SdkConfig,
    _: &str,
) -> anyhow::Result<()> {
    if apply {
        let acl = aws_sdk_s3::types::ObjectCannedAcl::from(object.acl.as_str());
        let body = ByteStream::from_path(&object.body.path)
            .await
            .with_context(|| {
                format!(
                    "could not create bytestream of '{}'",
                    object.body.path.display()
                )
            })?;
        let client = aws_sdk_s3::Client::new(cfg);
        client
            .put_object()
            .bucket(object.bucket.as_str())
            .acl(acl)
            .key(object.key.as_str())
            .body(body)
            .send()
            .await?;
    }
    Ok(())
}

async fn update_object(
    _object: &mut Object,
    apply: bool,
    _cfg: &SdkConfig,
    _name: &str,
    _previous: &Object,
) -> anyhow::Result<()> {
    if apply {
        unreachable!("object should be recreated");
    }

    Ok(())
}

async fn delete_object(
    object: &Object,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_s3::Client::new(cfg);
        client
            .delete_object()
            .bucket(object.bucket.as_str())
            .key(object.key.as_str())
            .send()
            .await?;
    }
    Ok(())
}
