//! Teleform for AWS.

pub use aws_config::SdkConfig;
pub mod apigatewayv2;
pub mod acm;
pub mod dynamodb;
pub mod iam;
pub mod lambda;
pub mod route53;
pub mod s3;

/// A wrapper around the AWS `SdkConfig` that provides `AsRef<SdkConfig>`.
pub struct Aws(pub SdkConfig);

impl AsRef<SdkConfig> for Aws {
    fn as_ref(&self) -> &SdkConfig {
        &self.0
    }
}

pub async fn prune<T: AsRef<SdkConfig>>(
    store: &mut crate::Store<T>
) -> anyhow::Result<()> {
    store.prune::<apigatewayv2::Route>().await?;
    store.prune::<apigatewayv2::Stage>().await?;
    store.prune::<apigatewayv2::Integration>().await?;
    store.prune::<apigatewayv2::ApiGatewayV2>().await?;
    store.prune::<dynamodb::Table>().await?;
    store.prune::<lambda::LambdaAddedPermission>().await?;
    store.prune::<lambda::Lambda>().await?;
    store.prune::<s3::Object>().await?;
    store.prune::<s3::Bucket>().await?;
    store.prune::<iam::Role>().await?;
    store.prune::<iam::Policy>().await?;
    Ok(())
}
