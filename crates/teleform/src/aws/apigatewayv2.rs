//! ApiGatewayV2 infrastructure.
use anyhow::Context;
use aws_config::SdkConfig;

use crate::{Local, Remote, TeleSync, self as tele};

#[derive(Debug, Default, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Protocol {
    #[default]
    Http,
    Websocket,
}

impl Protocol {
    fn as_str(&self) -> &str {
        match self {
            Protocol::Http => "HTTP",
            Protocol::Websocket => "WEBSOCKET",
        }
    }
}

#[derive(TeleSync, Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = &'a SdkConfig)]
#[tele(create = create_api, update = update_api, delete = delete_api)]
pub struct ApiGatewayV2 {
    pub target_lambda_arn: Option<Remote<String>>,
    pub protocol: Local<Protocol>,
    pub api_id: Remote<String>,
    pub api_endpoint: Remote<String>,
}

async fn create_api(
    api: &mut ApiGatewayV2,
    apply: bool,
    cfg: &SdkConfig,
    name: &str,
) -> anyhow::Result<()> {
    if apply {
        let protocol =
            aws_sdk_apigatewayv2::types::ProtocolType::from(api.protocol.as_ref().as_str());
        let client = aws_sdk_apigatewayv2::Client::new(cfg);
        let out = client
            .create_api()
            .name(name)
            .protocol_type(protocol)
            .set_target(
                api.target_lambda_arn
                    .as_ref()
                    .map(|arn| arn.maybe_ref())
                    .flatten()
                    .cloned(),
            )
            .send()
            .await?;
        api.api_id = out.api_id.context("missing api_id")?.into();
        api.api_endpoint = out.api_endpoint.context("missing api_endpoint")?.into();
    }
    Ok(())
}

async fn update_api(
    _: &mut ApiGatewayV2,
    apply: bool,
    cfg: &SdkConfig,
    _: &str,
    _: &ApiGatewayV2,
) -> anyhow::Result<()> {
    if apply {
        let _client = aws_sdk_apigatewayv2::Client::new(cfg);
        todo!()
    }

    Ok(())
}

async fn delete_api(
    api: &ApiGatewayV2,
    apply: bool,
    cfg: &SdkConfig,
    _: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_apigatewayv2::Client::new(cfg);
        client
            .delete_api()
            .api_id(
                api.api_id
                    .maybe_ref()
                    .context("cannot delete api - missing id")?,
            )
            .send()
            .await?;
    }
    Ok(())
}

#[derive(Debug, Default, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IntegrationType {
    Aws,
    #[default]
    AwsProxy,
    Http,
    HttpProxy,
    Mock,
}

impl From<IntegrationType> for aws_sdk_apigatewayv2::types::IntegrationType {
    fn from(value: IntegrationType) -> Self {
        match value {
            IntegrationType::Aws => aws_sdk_apigatewayv2::types::IntegrationType::Aws,
            IntegrationType::AwsProxy => aws_sdk_apigatewayv2::types::IntegrationType::AwsProxy,
            IntegrationType::Http => aws_sdk_apigatewayv2::types::IntegrationType::Http,
            IntegrationType::HttpProxy => aws_sdk_apigatewayv2::types::IntegrationType::HttpProxy,
            IntegrationType::Mock => aws_sdk_apigatewayv2::types::IntegrationType::Mock,
        }
    }
}

#[derive(TeleSync, Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = &'a SdkConfig)]
#[tele(create = create_integration, update = update_integration, delete = delete_integration)]
pub struct Integration {
    pub api_id: Remote<String>,
    // ARN of the lambda to integrate
    pub integration_uri: Remote<String>,
    pub integration_type: Local<IntegrationType>,
    // Known after creation
    pub integration_id: Remote<String>,
}

async fn create_integration(
    integration: &mut Integration,
    apply: bool,
    cfg: &SdkConfig,
    _: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_apigatewayv2::Client::new(cfg);
        let out =
            client
                .create_integration()
                .api_id(
                    integration
                        .api_id
                        .maybe_ref()
                        .context("cannot create integration - missing api_id")?,
                )
                .integration_type(integration.integration_type.0.into())
                .integration_uri(integration.integration_uri.maybe_ref().context(
                    "cannot create integration - missing integration_uri (aka lambda ARN)",
                )?)
                .payload_format_version("2.0")
                .send()
                .await?;
        integration.integration_id = out.integration_id.context("missing integration_id")?.into();
    }
    Ok(())
}

async fn update_integration(
    integration: &mut Integration,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
    _previous: &Integration,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_apigatewayv2::Client::new(cfg);
        let _out =
            client
                .update_integration()
                .integration_id(
                    integration
                        .integration_id
                        .maybe_ref()
                        .context("cannot update integration - missing integration_id")?,
                )
                .api_id(
                    integration
                        .api_id
                        .maybe_ref()
                        .context("cannot update integration - missing api_id")?,
                )
                .integration_type(integration.integration_type.0.into())
                .integration_uri(integration.integration_uri.maybe_ref().context(
                    "cannot create integration - missing integration_uri (aka lambda ARN)",
                )?)
                .send()
                .await?;
    }

    Ok(())
}

async fn delete_integration(
    integration: &Integration,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_apigatewayv2::Client::new(cfg);
        let _ = client
            .delete_integration()
            .integration_id(
                integration
                    .integration_id
                    .maybe_ref()
                    .context("cannot delete integration - missing integration_id")?,
            )
            .api_id(
                integration
                    .api_id
                    .maybe_ref()
                    .context("cannot delete integration - missing api_id")?,
            )
            .send()
            .await?;
    }
    Ok(())
}

#[derive(TeleSync, Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = &'a SdkConfig)]
#[tele(create = create_route, update = update_route, delete = delete_route)]
pub struct Route {
    // Id of the ApiGatewayV2 gateway
    pub api_id: Remote<String>,
    // Method and name of the route, eg "POST /pets", "ANY /cousins"
    pub route_key: Local<String>,
    // Integration id
    pub target: Remote<Option<String>>,
    // Known after creation.
    pub route_id: Remote<String>,
}

async fn create_route(
    route: &mut Route,
    apply: bool,
    cfg: &SdkConfig,
    _: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_apigatewayv2::Client::new(cfg);
        let out = client
            .create_route()
            .api_id(
                route
                    .api_id
                    .maybe_ref()
                    .context("cannot create route - missing api_id")?,
            )
            .route_key(route.route_key.as_str())
            .set_target(
                route
                    .target
                    .maybe_ref()
                    .map(Option::as_ref)
                    .flatten()
                    .cloned(),
            )
            .send()
            .await?;
        route.route_id = out.route_id.context("missing route_id")?.into();
    }
    Ok(())
}

async fn update_route(
    route: &mut Route,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
    _previous: &Route,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_apigatewayv2::Client::new(cfg);
        let out = client
            .update_route()
            .route_id(
                route
                    .route_id
                    .maybe_ref()
                    .context("cannot update route - missing route_id")?,
            )
            .api_id(
                route
                    .api_id
                    .maybe_ref()
                    .context("cannot create route - missing api_id")?,
            )
            .route_key(route.route_key.as_str())
            .set_target(
                route
                    .target
                    .maybe_ref()
                    .map(Option::as_ref)
                    .flatten()
                    .cloned(),
            )
            .send()
            .await?;
        route.route_id = out.route_id.context("missing route_id")?.into();
    }

    Ok(())
}

async fn delete_route(
    route: &Route,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_apigatewayv2::Client::new(cfg);
        let _ = client
            .delete_route()
            .api_id(
                route
                    .api_id
                    .maybe_ref()
                    .context("cannot delete route - missing api_id")?,
            )
            .route_id(
                route
                    .route_id
                    .maybe_ref()
                    .context("cannot delete route - missing route_id")?,
            )
            .send()
            .await?;
    }
    Ok(())
}

#[derive(TeleSync, Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[tele(helper = &'a SdkConfig)]
#[tele(create = create_stage, update = update_stage, delete = delete_stage)]
pub struct Stage {
    #[tele(should_recreate)]
    pub api_id: Remote<String>,
    pub stage_name: Local<String>,
    pub auto_deploy: Local<bool>,
}

async fn create_stage(
    stage: &mut Stage,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_apigatewayv2::Client::new(cfg);
        let _ = client
            .create_stage()
            .api_id(stage.api_id.maybe_ref().context("cannot create stage - missing api_id")?)
            .stage_name(stage.stage_name.as_str())
            .auto_deploy(*stage.auto_deploy.as_ref())
            .send()
            .await?;
    }
    Ok(())
}

async fn update_stage(
    stage: &mut Stage,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
    _previous: &Stage,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_apigatewayv2::Client::new(cfg);
        client
            .update_stage()
            .api_id(stage.api_id.maybe_ref().context("cannot update stage - missing api_id")?)
            .stage_name(stage.stage_name.as_str())
            .auto_deploy(*stage.auto_deploy)
            .send()
            .await?;
    }

    Ok(())
}

async fn delete_stage(
    stage: &Stage,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_apigatewayv2::Client::new(cfg);
        let _ = client
            .delete_stage()
            .api_id(stage.api_id.maybe_ref().context("cannot delete stage - missing api_id")?)
            .stage_name(stage.stage_name.as_str())
            .send()
            .await?;
    }
    Ok(())
}
