//
// Copyright 2026 Signal Messenger, LLC.
// SPDX-License-Identifier: AGPL-3.0-only
//

use std::convert::Infallible;
use std::time::SystemTime;

use async_trait::async_trait;
use libsignal_core::{Aci, ServiceId};
use libsignal_net_grpc::proto::chat::account::accounts_anonymous_client::AccountsAnonymousClient;
use libsignal_net_grpc::proto::chat::account::{
    CheckAccountExistenceRequest, CheckAccountExistenceResponse,
};
use libsignal_net_grpc::proto::chat::errors;
use libsignal_net_grpc::proto::chat::profile::get_profile_key_credential_response::Response as GetProfileKeyCredentialResponseEnum;
use libsignal_net_grpc::proto::chat::profile::profile_anonymous_client::ProfileAnonymousClient;
use libsignal_net_grpc::proto::chat::profile::{
    CredentialType, GetProfileKeyCredentialRequest, GetProfileKeyCredentialResponse,
    GetProfileKeyCredentialResult,
};

use crate::api::profiles::ProfileKeyCredentialRequestError;
use crate::api::{RequestError, Unauth};
use crate::grpc::{GrpcServiceProvider, OverGrpc, log_and_send};
use crate::logging::Redact;

impl std::fmt::Display for Redact<CheckAccountExistenceRequest> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(CheckAccountExistenceRequest { service_identifier }) = self;
        f.debug_struct("CheckAccountExistenceRequest")
            .field(
                "service_identifier",
                &service_identifier.as_ref().map(Redact),
            )
            .finish()
    }
}

impl std::fmt::Display for Redact<GetProfileKeyCredentialRequest> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(GetProfileKeyCredentialRequest {
            account_identifier,
            credential_request,
            credential_type,
            // This is fixed-length, somewhat sensitive, and not helpful for debugging.
            unidentified_access_key: _,
        }) = self;
        f.debug_struct("GetProfileKeyCredentialRequest")
            .field("aci", &account_identifier.as_ref().map(Redact))
            .field("credential_type", &credential_type)
            .field("credential_request.len()", &credential_request.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl<T: GrpcServiceProvider> crate::api::profiles::UnauthenticatedAccountExistenceApi<OverGrpc>
    for Unauth<T>
{
    async fn account_exists(&self, account: ServiceId) -> Result<bool, RequestError<Infallible>> {
        let mut account_service = AccountsAnonymousClient::new(self.0.service());
        let request = CheckAccountExistenceRequest {
            service_identifier: Some(account.into()),
        };
        let log_safe_description = Redact(&request).to_string();
        let CheckAccountExistenceResponse { account_exists } =
            log_and_send(Self::LOG_TAG, &log_safe_description, || {
                account_service.check_account_existence(request)
            })
            .await?
            .into_inner();
        Ok(account_exists)
    }
}

impl<T: GrpcServiceProvider> Unauth<T> {
    pub async fn get_profile_key_credential(
        &self,
        profile_key_request_context: zkgroup::profiles::ProfileKeyCredentialRequestContext,
        server_params: &zkgroup::ServerPublicParams,
    ) -> Result<
        zkgroup::profiles::ExpiringProfileKeyCredential,
        RequestError<ProfileKeyCredentialRequestError>,
    > {
        let request = GetProfileKeyCredentialRequest {
            account_identifier: Some(profile_key_request_context.aci().into()),
            credential_request: zkgroup::serialize(&profile_key_request_context.get_request()),
            credential_type: CredentialType::ExpiringProfileKey.into(),
            unidentified_access_key: profile_key_request_context
                .profile_key()
                .derive_access_key()
                .to_vec(),
        };
        let log_safe_description = Redact(&request).to_string();

        let mut profiles_service = ProfileAnonymousClient::new(self.0.service());
        let GetProfileKeyCredentialResponse { response } =
            log_and_send(Self::LOG_TAG, &log_safe_description, || {
                profiles_service.get_profile_key_credential(request)
            })
            .await?
            .into_inner();
        let response = response.ok_or_else(|| RequestError::Unexpected {
            log_safe: "missing response".to_owned(),
        })?;

        match response {
            GetProfileKeyCredentialResponseEnum::Result(GetProfileKeyCredentialResult {
                profile_key_credential,
            }) => {
                let zk_response = zkgroup::deserialize(&profile_key_credential).map_err(|e| {
                    RequestError::Unexpected {
                        log_safe: e.to_string(),
                    }
                })?;
                server_params
                    .receive_expiring_profile_key_credential(
                        &profile_key_request_context,
                        &zk_response,
                        SystemTime::now().into(),
                    )
                    .map_err(|e| RequestError::Unexpected {
                        log_safe: e.to_string(),
                    })
            }
            GetProfileKeyCredentialResponseEnum::NotFound(errors::NotFound {}) => Err(
                RequestError::Other(ProfileKeyCredentialRequestError::ProfileNotFound),
            ),
            GetProfileKeyCredentialResponseEnum::FailedUnidentifiedAuthorization(
                errors::FailedUnidentifiedAuthorization { description },
            ) => {
                log::warn!("failed auth: {description}");
                Err(RequestError::Other(
                    ProfileKeyCredentialRequestError::AuthFailed,
                ))
            }
        }
    }
}

pub mod test_cases {
    use libsignal_net_grpc::proto::chat::common::{IdentityType, ServiceIdentifier};
    use uuid::{Uuid, uuid};
    use zkgroup::SECONDS_PER_DAY;

    use super::*;
    use crate::grpc::GrpcTestCase;
    use crate::grpc::test_case_util::day_align;

    pub(crate) const ACI_UUID: Uuid = uuid!("9d0652a3-dcc3-4d11-975f-74d61598733f");
    #[cfg(test)]
    pub(crate) const PNI_UUID: Uuid = uuid!("796abedb-ca4e-4f18-8803-1fde5b921f9f");

    #[derive(Clone)]
    pub struct GetProfileKeyCredentialArgs {
        pub profile_key_request_context: zkgroup::profiles::ProfileKeyCredentialRequestContext,
        pub server_params: zkgroup::ServerPublicParams,
    }
    #[allow(clippy::large_enum_variant)]
    pub enum GetProfileKeyCredentialOut {
        Success(zkgroup::profiles::ExpiringProfileKeyCredential),
        UnexpectedError { contains: &'static str },
        ExplicitError(ProfileKeyCredentialRequestError),
    }

    pub fn get_profile_key_credential_test_cases() -> Vec<
        GrpcTestCase<
            GetProfileKeyCredentialArgs,
            GetProfileKeyCredentialRequest,
            GetProfileKeyCredentialResponse,
            GetProfileKeyCredentialOut,
        >,
    > {
        let method = "/org.signal.chat.profile.ProfileAnonymous/GetProfileKeyCredential";
        let profile_key = zkgroup::profiles::ProfileKey::create([b'p'; zkgroup::PROFILE_KEY_LEN]);
        let commitment = profile_key.get_commitment(Aci::from(ACI_UUID));
        let server_secret_params =
            zkgroup::ServerSecretParams::generate([1; zkgroup::RANDOMNESS_LEN]);
        let server_params = server_secret_params.get_public_params();
        let request_context = server_params.create_profile_key_credential_request_context(
            [2; zkgroup::RANDOMNESS_LEN],
            Aci::from(ACI_UUID),
            profile_key,
        );
        let start_of_today = day_align(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("time goes forward")
                .as_secs(),
        );
        let zk_response = server_secret_params
            .issue_expiring_profile_key_credential(
                [3; zkgroup::RANDOMNESS_LEN],
                &request_context.get_request(),
                request_context.aci(),
                commitment,
                zkgroup::Timestamp::from_epoch_seconds(start_of_today + 2 * SECONDS_PER_DAY),
            )
            .expect("valid");
        let credential = server_params
            .receive_expiring_profile_key_credential(
                &request_context,
                &zk_response,
                SystemTime::now().into(),
            )
            .expect("valid");

        let case = |name: &str, response_grpc, response| GrpcTestCase {
            name: name.to_owned(),
            method: method.into(),
            request: GetProfileKeyCredentialArgs {
                profile_key_request_context: request_context.clone(),
                server_params: server_params.clone(),
            },
            request_grpc: GetProfileKeyCredentialRequest {
                account_identifier: Some(ServiceIdentifier {
                    identity_type: IdentityType::Aci.into(),
                    uuid: ACI_UUID.into_bytes().to_vec(),
                }),
                credential_request: zkgroup::serialize(&request_context.get_request()),
                credential_type: CredentialType::ExpiringProfileKey.into(),
                unidentified_access_key: profile_key.derive_access_key().to_vec(),
            },
            response_grpc,
            response,
        };

        vec![
            case(
                "success",
                GetProfileKeyCredentialResponse {
                    response: Some(GetProfileKeyCredentialResponseEnum::Result(
                        GetProfileKeyCredentialResult {
                            profile_key_credential: zkgroup::serialize(&zk_response),
                        },
                    )),
                },
                GetProfileKeyCredentialOut::Success(credential),
            ),
            case(
                "missing response",
                GetProfileKeyCredentialResponse { response: None },
                GetProfileKeyCredentialOut::UnexpectedError {
                    contains: "missing response",
                },
            ),
            case(
                "garbage credential",
                GetProfileKeyCredentialResponse {
                    response: Some(GetProfileKeyCredentialResponseEnum::Result(
                        GetProfileKeyCredentialResult {
                            profile_key_credential: vec![],
                        },
                    )),
                },
                GetProfileKeyCredentialOut::UnexpectedError {
                    contains: "Failed to deserialize",
                },
            ),
            case(
                "bad expiration",
                GetProfileKeyCredentialResponse {
                    response: Some(GetProfileKeyCredentialResponseEnum::Result(
                        GetProfileKeyCredentialResult {
                            profile_key_credential: zkgroup::serialize(
                                &server_secret_params
                                    .issue_expiring_profile_key_credential(
                                        [3; zkgroup::RANDOMNESS_LEN],
                                        &request_context.get_request(),
                                        request_context.aci(),
                                        commitment,
                                        zkgroup::Timestamp::from_epoch_seconds(
                                            start_of_today + 2 * SECONDS_PER_DAY + 1,
                                        ),
                                    )
                                    .expect("valid"),
                            ),
                        },
                    )),
                },
                GetProfileKeyCredentialOut::UnexpectedError {
                    contains: "Verification failure",
                },
            ),
            case(
                "not found",
                GetProfileKeyCredentialResponse {
                    response: Some(GetProfileKeyCredentialResponseEnum::NotFound(
                        errors::NotFound {},
                    )),
                },
                GetProfileKeyCredentialOut::ExplicitError(
                    ProfileKeyCredentialRequestError::ProfileNotFound,
                ),
            ),
            case(
                "auth failed",
                GetProfileKeyCredentialResponse {
                    response: Some(
                        GetProfileKeyCredentialResponseEnum::FailedUnidentifiedAuthorization(
                            errors::FailedUnidentifiedAuthorization {
                                description: "bad".to_owned(),
                            },
                        ),
                    ),
                },
                GetProfileKeyCredentialOut::ExplicitError(
                    ProfileKeyCredentialRequestError::AuthFailed,
                ),
            ),
        ]
    }
}

#[cfg(test)]
mod test {
    use assert_matches::assert_matches;
    use futures_util::FutureExt;
    use libsignal_core::{Aci, Pni};
    use libsignal_net_grpc::proto::chat::services;
    use test_case::test_matrix;

    use super::test_cases::*;
    use super::*;
    use crate::api::profiles::UnauthenticatedAccountExistenceApi;
    use crate::grpc::testutil::{
        GrpcOverrideRequestValidator, RequestValidator, err, ok, req, run_tests,
    };

    #[test_matrix([Aci::from(ACI_UUID).into(), Pni::from(PNI_UUID).into()], [false, true])]
    fn test_account_exists(service_id: ServiceId, found: bool) {
        let validator = GrpcOverrideRequestValidator {
            message: services::AccountsAnonymous::CheckAccountExistence.into(),
            validator: RequestValidator {
                expected: req(
                    "/org.signal.chat.account.AccountsAnonymous/CheckAccountExistence",
                    CheckAccountExistenceRequest {
                        service_identifier: Some(service_id.into()),
                    },
                ),
                response: ok(CheckAccountExistenceResponse {
                    account_exists: found,
                }),
            },
        };
        let result = Unauth(&validator)
            .account_exists(service_id)
            .now_or_never()
            .expect("sync")
            .expect("success");
        assert_eq!(result, found);
    }

    #[test]
    fn test_account_exists_invalid() {
        let validator = GrpcOverrideRequestValidator {
            message: services::AccountsAnonymous::CheckAccountExistence.into(),
            validator: RequestValidator {
                expected: req(
                    "/org.signal.chat.account.AccountsAnonymous/CheckAccountExistence",
                    CheckAccountExistenceRequest {
                        service_identifier: Some(Aci::from(ACI_UUID).into()),
                    },
                ),
                response: err(tonic::Code::DeadlineExceeded),
            },
        };
        let result = Unauth(&validator)
            .account_exists(Aci::from(ACI_UUID).into())
            .now_or_never()
            .expect("sync")
            .expect_err("should fail");
        assert_matches!(result, RequestError::Timeout);
    }

    #[test]
    fn test_get_profile_key_credential() {
        run_tests(
            test_cases::get_profile_key_credential_test_cases(),
            |chat: Unauth<_>,
             GetProfileKeyCredentialArgs {
                 profile_key_request_context,
                 server_params,
             }| async move {
                chat.get_profile_key_credential(profile_key_request_context, &server_params)
                    .await
            },
            |expected, actual| match expected {
                GetProfileKeyCredentialOut::Success(expected_credential) => assert_eq!(
                    zkgroup::serialize(&expected_credential),
                    zkgroup::serialize(&actual.expect("success"))
                ),
                GetProfileKeyCredentialOut::UnexpectedError { contains } => {
                    let Err(err) = actual else {
                        panic!("should have failed");
                    };
                    assert_matches!(
                        err,
                        RequestError::Unexpected { log_safe }
                        if log_safe.contains(contains)
                    );
                }
                GetProfileKeyCredentialOut::ExplicitError(expected_err) => {
                    let Err(err) = actual else {
                        panic!("should have failed");
                    };
                    assert_eq!(
                        expected_err,
                        assert_matches!(err, RequestError::Other(e) => e)
                    );
                }
            },
        );
    }
}
