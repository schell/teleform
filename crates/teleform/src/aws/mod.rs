//! Teleform for AWS.

pub use aws_config::SdkConfig;
pub mod apigatewayv2;
pub mod dynamodb;
pub mod iam;
pub mod lambda;
pub mod s3;

pub async fn prune<'a: 'b, 'b>(
    store: &'b mut crate::Store<&'a SdkConfig>
) -> anyhow::Result<()> {
    store.prune::<apigatewayv2::Route>().await?;
    store.prune::<apigatewayv2::Integration>().await?;
    store.prune::<apigatewayv2::ApiGatewayV2>().await?;
    store.prune::<dynamodb::Table>().await?;
    store.prune::<lambda::LambdaAddedPermission>().await?;
    store.prune::<lambda::Lambda>().await?;
    store.prune::<s3::Bucket>().await?;
    store.prune::<iam::Role>().await?;
    store.prune::<iam::Policy>().await?;
    Ok(())
}
