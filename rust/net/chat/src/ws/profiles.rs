//
// Copyright 2025 Signal Messenger, LLC.
// SPDX-License-Identifier: AGPL-3.0-only
//

use std::convert::Infallible;

use async_trait::async_trait;
use http::HeaderMap;
use libsignal_core::ServiceId;
use libsignal_net::chat::Request;
use libsignal_net_grpc::proto::chat::services;

use super::{CustomError, OverWs, ResponseError, WsConnection};
use crate::api::{RequestError, Unauth};
use crate::logging::Redact;

#[async_trait]
impl<T: WsConnection> crate::api::profiles::UnauthenticatedAccountExistenceApi<OverWs>
    for Unauth<T>
{
    async fn account_exists(&self, account: ServiceId) -> Result<bool, RequestError<Infallible>> {
        if let Some(grpc) = self
            .grpc_service_to_use_instead(services::AccountsAnonymous::CheckAccountExistence.into())
        {
            return Unauth(grpc).account_exists(account).await;
        }
        let log_safe_path = format!("/v1/accounts/account/{}", Redact(&account));
        let response = self
            .send(
                Self::LOG_TAG,
                &log_safe_path,
                Request {
                    method: http::Method::HEAD,
                    path: format!("/v1/accounts/account/{}", account.service_id_string(),)
                        .parse()
                        .expect("valid"),
                    headers: HeaderMap::default(),
                    body: None,
                },
            )
            .await?;
        if let Some(body) = response.body.as_ref()
            && !body.is_empty()
        {
            log::warn!("HEAD {log_safe_path} returned a non-empty body");
        }
        match response.status {
            http::status::StatusCode::OK => Ok(true),
            http::status::StatusCode::NOT_FOUND => Ok(false),
            status => Err(ResponseError::UnrecognizedStatus { status, response }
                .into_request_error(
                    Self::ALLOW_RATE_LIMIT_CHALLENGES,
                    CustomError::no_custom_handling,
                )),
        }
    }
}

#[cfg(test)]
mod test_account_exists {
    use futures_util::FutureExt;
    use libsignal_core::{Aci, Pni};
    use libsignal_net::chat::Response;
    use test_case::test_case;
    use uuid::{Uuid, uuid};

    use super::*;
    use crate::api::profiles::UnauthenticatedAccountExistenceApi;
    use crate::ws::testutil::RequestValidator;

    const ACI_UUID: Uuid = uuid!("9d0652a3-dcc3-4d11-975f-74d61598733f");
    const PNI_UUID: Uuid = uuid!("796abedb-ca4e-4f18-8803-1fde5b921f9f");

    #[test_case(Aci::from(ACI_UUID).into(), true)]
    #[test_case(Pni::from(PNI_UUID).into(), true)]
    #[test_case(Aci::from(ACI_UUID).into(), false)]
    #[test_case(Pni::from(PNI_UUID).into(), false)]
    #[tokio::test]
    async fn test_it(service_id: ServiceId, found: bool) {
        let validator = RequestValidator {
            expected: Request {
                method: http::Method::HEAD,
                path: format!("/v1/accounts/account/{}", service_id.service_id_string())
                    .parse()
                    .expect("valid"),
                headers: Default::default(),
                body: None,
            },
            response: Response {
                status: if found {
                    http::StatusCode::OK
                } else {
                    http::StatusCode::NOT_FOUND
                },
                message: None,
                headers: Default::default(),
                body: None,
            },
        };
        let result = Unauth(validator)
            .account_exists(service_id)
            .now_or_never()
            .expect("sync")
            .expect("success");
        assert_eq!(result, found);
    }
}
