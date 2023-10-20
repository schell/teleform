use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use aws_sdk_dynamodb::types::AttributeValue;
use anyhow::Context;

/// This is the main body for the function.
/// Write your code inside it.
/// There are some code example in the following URLs:
/// - https://github.com/awslabs/aws-lambda-rust-runtime/tree/main/examples
async fn function_handler(
    client: &aws_sdk_dynamodb::Client,
    event: Request,
) -> Result<Response<Body>, Error> {
    // Extract some useful information from the request
    let who = event
        .query_string_parameters_ref()
        .and_then(|params| params.first("name"))
        .unwrap_or("world");

    const TABLE_NAME: &str = "crud-table";

    let out = client
        .get_item()
        .table_name(TABLE_NAME)
        .key("id", AttributeValue::S(who.to_string()))
        .projection_expression("num")
        .send()
        .await?;
    let n = if let Some(item) = out.item {
        // increment a count
        let att = item
            .get("num")
            .context("no such attribute 'num'")?;
        let count_str = att.as_n().ok().context("'num' is not a number")?;
        let count = count_str.parse::<u32>().context("cannot parse 'num'")? + 1;
        client
            .put_item()
            .table_name(TABLE_NAME)
            .item("id", AttributeValue::S(who.to_string()))
            .item("num", AttributeValue::N(count.to_string()))
            .send()
            .await
            .context("could not put item 'num'")?;
        count
    } else {
        // insert the item
        client
            .put_item()
            .table_name(TABLE_NAME)
            .item("id", AttributeValue::S(who.to_string()))
            .item("num", AttributeValue::N(1.to_string()))
            .send()
            .await
            .context("could not new put item 'num'")?;
        1
    };

    let message = format!("Hello {who}, this is request {n}");

    // Return something that implements IntoResponse.
    // It will be serialized to the right response event automatically by the runtime
    let resp = Response::builder()
        .status(200)
        .header("content-type", "text/html")
        .body(message.into())
        .map_err(Box::new)?;
    Ok(resp)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        // disable printing the name of the module in every log line.
        .with_target(false)
        // disabling time is handy because CloudWatch will add the ingestion time.
        .without_time()
        .init();
    let cfg = aws_config::from_env().load().await;
    let client = aws_sdk_dynamodb::Client::new(&cfg);
    run(service_fn(|event| function_handler(&client, event))).await
}
