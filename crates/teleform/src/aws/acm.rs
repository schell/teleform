//! AWS certificate manager infrastructure.
use std::collections::HashSet;

use anyhow::Context;
use aws_config::SdkConfig;
use aws_sdk_acm::types::{DomainStatus, RecordType};

use crate::{self as tele, Local, Remote, TeleSync};

#[derive(Debug, Default, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ValidationMethod {
    #[default]
    Dns,
    Email,
}

impl From<ValidationMethod> for aws_sdk_acm::types::ValidationMethod {
    fn from(value: ValidationMethod) -> Self {
        match value {
            ValidationMethod::Dns => aws_sdk_acm::types::ValidationMethod::Dns,
            ValidationMethod::Email => aws_sdk_acm::types::ValidationMethod::Email,
        }
    }
}

impl TryFrom<aws_sdk_acm::types::ValidationMethod> for ValidationMethod {
    type Error = anyhow::Error;

    fn try_from(value: aws_sdk_acm::types::ValidationMethod) -> Result<Self, Self::Error> {
        match value {
            aws_sdk_acm::types::ValidationMethod::Dns => Ok(ValidationMethod::Dns),
            aws_sdk_acm::types::ValidationMethod::Email => Ok(ValidationMethod::Email),
            m => anyhow::bail!("unsupported validation method {m:?}"),
        }
    }
}

#[derive(Debug)]
pub struct ValidationResourceRecord {
    pub name: String,
    pub type_is: RecordType,
    pub value: String,
}

#[derive(Debug)]
pub struct ValidationOption {
    pub validation_domain: String,
    pub validation_status: DomainStatus,
    pub validation_method: ValidationMethod,
    pub resource_record: ValidationResourceRecord,
}

/// AWS certificate.
#[derive(TeleSync, Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[tele(helper = SdkConfig)]
#[tele(create = create_cert, update = update_cert, delete = delete_cert)]
pub struct Certificate {
    // Fully qualified domain name.
    //
    // May contain wilcards like "*.zyghost.com".
    pub domain_name: Local<String>,
    pub validation_method: Local<ValidationMethod>,
    pub subject_alternative_names: Local<Vec<String>>,
    // Known after creation.
    pub arn: Remote<String>,
}

async fn create_cert(
    cert: &mut Certificate,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        let client = aws_sdk_acm::Client::new(cfg);
        let validation: aws_sdk_acm::types::ValidationMethod =
            (*cert.validation_method.as_ref()).into();
        let subject_alt_names = cert.subject_alternative_names.as_ref().clone();
        let subject_alt_names = if subject_alt_names.is_empty() {
            None
        } else {
            Some(subject_alt_names)
        };
        let out = client
            .request_certificate()
            .domain_name(cert.domain_name.as_str())
            .validation_method(validation)
            .set_subject_alternative_names(subject_alt_names)
            .send()
            .await?;
        cert.arn = out
            .certificate_arn
            .context("missing output certificate ARN")?
            .into();
    }
    Ok(())
}

async fn update_cert(
    _cert: &mut Certificate,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
    _previous: &Certificate,
) -> anyhow::Result<()> {
    if apply {
        let _client = aws_sdk_acm::Client::new(cfg);
        todo!()
    }

    Ok(())
}

async fn delete_cert(
    _cert: &Certificate,
    apply: bool,
    cfg: &SdkConfig,
    _name: &str,
) -> anyhow::Result<()> {
    if apply {
        let _client = aws_sdk_acm::Client::new(cfg);
        todo!()
    }
    Ok(())
}

impl Certificate {
    pub async fn get_validation_options(
        &self,
        cfg: &SdkConfig,
    ) -> anyhow::Result<Vec<ValidationOption>> {
        let client = aws_sdk_acm::Client::new(cfg);
        let out = client
            .describe_certificate()
            .certificate_arn(self.arn.maybe_ref().context("missing cert arn")?)
            .send()
            .await?;
        let cert_detail = out.certificate.context("missing cert detail")?;
        let validation_options = cert_detail
            .domain_validation_options
            .context("missing validation options")?;
        let mut options = vec![];
        let mut set = HashSet::<String>::default();
        for vo in validation_options.into_iter() {
            let r = vo.resource_record.context("missing record")?;
            let name = r.name;
            if set.contains(&name) {
                continue;
            } else {
                set.insert(name.clone());
            }
            let option = ValidationOption {
                validation_domain: vo.validation_domain.context("missing domain")?,
                validation_status: vo.validation_status.context("missing status")?,
                validation_method: vo.validation_method.context("missing method")?.try_into()?,
                resource_record: {
                    ValidationResourceRecord {
                        name,
                        type_is: r.r#type,
                        value: r.value,
                    }
                },
            };
            options.push(option);
        }
        Ok(options)
    }
}
