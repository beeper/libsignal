//
// Copyright 2026 Signal Messenger, LLC.
// SPDX-License-Identifier: AGPL-3.0-only
//

use std::time::{Duration, SystemTime};

use libsignal_net_grpc::proto::chat::errors::{FailedPrecondition, NotFound};
use libsignal_net_grpc::proto::chat::purchase::subscriptions_client::SubscriptionsClient;
use libsignal_net_grpc::proto::chat::purchase::{
    GetReceiptCredentialRequest, get_receipt_credential_response,
};
use zkgroup::receipts::{
    ReceiptCredential, ReceiptCredentialRequestContext, ReceiptCredentialResponse,
};
use zkgroup::{SECONDS_PER_DAY, ServerPublicParams, ZkGroupVerificationFailure};

use super::{GrpcServiceProvider, log_and_send};
use crate::api::purchase::ChargeFailure;
use crate::api::{RequestError, Unauth};
use crate::logging::{Redact, RedactBytesAsHex};

pub type SubscriberId = [u8; 32];

#[derive(Clone, Debug, displaydoc::Display)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub enum ReceiptCredentialError {
    /// No invoice has been issued for this subscription OR invoice is in 'draft' or 'open' state.
    NoPaidInvoice,
    /// The purchase did not complete successfully.
    PaymentRequired {
        charge_failure: Option<Box<ChargeFailure>>,
    },
    /// The subscriber did not exist or it did not have an associated subscription.
    SubscriberNotFound,
    /// The purchase was already redeemed for a receipt credential using a different request.
    ReceiptAlreadyIssued,
}

const UNEXPECTED_CANT_RECV: &str = "Failed to receive receipt credential response";
const UNEXPECTED_EXPIRATION_OUT_OF_RANGE: &str =
    "Invalid receipt: expiration time outside of range";

impl<T: GrpcServiceProvider> Unauth<T> {
    pub async fn get_subscription_receipt_credential(
        &self,
        subscriber_id: SubscriberId,
        receipt_credential_request_context: &ReceiptCredentialRequestContext,
        server_params: &ServerPublicParams,
    ) -> Result<ReceiptCredential, RequestError<ReceiptCredentialError>> {
        let mut client = SubscriptionsClient::new(self.0.service());
        let request = GetReceiptCredentialRequest {
            subscriber_id: subscriber_id.to_vec(),
            receipt_credential_request: zkgroup::serialize(
                &receipt_credential_request_context.get_request(),
            ),
        };
        let desc = Redact(&request).to_string();
        match log_and_send(Self::LOG_TAG, &desc, || {
            client.get_receipt_credential(request)
        })
        .await?
        .into_inner()
        .response
        .ok_or_else(|| RequestError::Unexpected {
            log_safe: "Missing response".to_string(),
        })? {
            get_receipt_credential_response::Response::Success(
                get_receipt_credential_response::GetReceiptCredentialResult {
                    receipt_credential_response,
                },
            ) => {
                let response: ReceiptCredentialResponse =
                    zkgroup::deserialize(&receipt_credential_response).map_err(|_| {
                        RequestError::Unexpected {
                            log_safe: "Can't deserialize receipt credential response".into(),
                        }
                    })?;
                let out = server_params
                    .receive_receipt_credential(receipt_credential_request_context, &response)
                    .map_err(|ZkGroupVerificationFailure| RequestError::Unexpected {
                        log_safe: UNEXPECTED_CANT_RECV.into(),
                    })?;
                if !out
                    .get_receipt_expiration_time()
                    .epoch_seconds()
                    .is_multiple_of(SECONDS_PER_DAY)
                {
                    return Err(RequestError::Unexpected {
                        log_safe: "Invalid receipt: expiration time".into(),
                    });
                }
                // Check the expiration:
                // - must not be more than 24 hours ago
                // - must not be more than 90 days in the future
                let yesterday_at_this_time =
                    SystemTime::now() - Duration::from_secs(SECONDS_PER_DAY);
                let expiration_time = SystemTime::UNIX_EPOCH
                    + Duration::from_secs(out.get_receipt_expiration_time().epoch_seconds());
                if !expiration_time
                    .duration_since(yesterday_at_this_time)
                    .is_ok_and(|duration| duration < Duration::from_secs(91 * SECONDS_PER_DAY))
                {
                    return Err(RequestError::Unexpected {
                        log_safe: UNEXPECTED_EXPIRATION_OUT_OF_RANGE.into(),
                    });
                }
                Ok(out)
            }
            get_receipt_credential_response::Response::SubscriberNotFound(NotFound {}) => Err(
                RequestError::Other(ReceiptCredentialError::SubscriberNotFound),
            ),
            get_receipt_credential_response::Response::NoPaidInvoice(FailedPrecondition {
                description,
            }) => {
                log::warn!("NoPaidInvoice: {description}");
                Err(RequestError::Other(ReceiptCredentialError::NoPaidInvoice))
            }
            get_receipt_credential_response::Response::PaymentRequired(payment_required) => Err(
                RequestError::Other(ReceiptCredentialError::PaymentRequired {
                    charge_failure: payment_required
                        .charge_failure
                        .map(|charge_failure| Ok(Box::new(charge_failure.try_into()?)))
                        .transpose()
                        .map_err(RequestError::with_other)?,
                }),
            ),
            get_receipt_credential_response::Response::AlreadyRedeemed(FailedPrecondition {
                description,
            }) => {
                log::warn!("AlreadyRedeemed {description}");
                Err(RequestError::Other(
                    ReceiptCredentialError::ReceiptAlreadyIssued,
                ))
            }
        }
    }
}

impl std::fmt::Display for Redact<GetReceiptCredentialRequest> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let GetReceiptCredentialRequest {
            subscriber_id,
            receipt_credential_request,
        } = &self.0;
        f.debug_struct("GetReceiptCredentialRequest")
            .field("subscriber_id", &RedactBytesAsHex(subscriber_id))
            .field(
                "receipt_credential_request.len()",
                &receipt_credential_request.len(),
            )
            .finish()
    }
}

pub mod test_cases {
    use libsignal_net_grpc::proto::chat::errors::{FailedPrecondition, NotFound};
    use libsignal_net_grpc::proto::chat::purchase::get_receipt_credential_response::GetReceiptCredentialResult;
    use libsignal_net_grpc::proto::chat::purchase::{
        ChargeFailure as GrpcChargeFailure, GetReceiptCredentialResponse,
        PaymentProvider as GrpcPaymentProvider,
    };
    use zkgroup::ServerSecretParams;

    use super::*;
    use crate::api::purchase::PaymentProvider;
    use crate::grpc::GrpcTestCase;

    fn day_align(x: u64) -> u64 {
        (x / SECONDS_PER_DAY) * SECONDS_PER_DAY
    }

    #[derive(Clone)]
    pub struct GetReceiptCredentialArgs {
        pub subscriber_id: SubscriberId,
        pub receipt_credential_request_context: ReceiptCredentialRequestContext,
        pub server_params: ServerPublicParams,
    }
    #[allow(clippy::large_enum_variant)]
    pub enum GetReceiptCredentialOut {
        Success(ReceiptCredential),
        UnexpectedError { contains: String },
        ExplicitError(ReceiptCredentialError),
    }
    pub fn get_receipt_credential_test_cases() -> Vec<
        GrpcTestCase<
            GetReceiptCredentialArgs,
            GetReceiptCredentialRequest,
            GetReceiptCredentialResponse,
            GetReceiptCredentialOut,
        >,
    > {
        let server_secret_params = ServerSecretParams::generate([0x01; _]);
        let server_params = server_secret_params.get_public_params();
        let ctx = server_params.create_receipt_credential_request_context([0x02; _], [0x04; _]);
        let method = "/org.signal.chat.purchase.Subscriptions/GetReceiptCredential";
        let subscriber_id = [b's'; 32];
        let issue_receipt = |level, expiration| {
            server_secret_params.issue_receipt_credential(
                [0x5; _],
                &ctx.get_request(),
                expiration,
                level,
            )
        };
        let request = GetReceiptCredentialArgs {
            subscriber_id,
            receipt_credential_request_context: ctx.clone(),
            server_params: server_params.clone(),
        };
        let grpc_request = GetReceiptCredentialRequest {
            subscriber_id: subscriber_id.into(),
            receipt_credential_request: zkgroup::serialize(&ctx.get_request()),
        };
        let now_seconds = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("present day")
            .as_secs();
        let valid_response = issue_receipt(
            200,
            zkgroup::Timestamp::from_epoch_seconds(day_align(now_seconds + 3 * SECONDS_PER_DAY)),
        );
        let just_barely_expired_response = issue_receipt(
            200,
            zkgroup::Timestamp::from_epoch_seconds(day_align(now_seconds)),
        );
        let mut test_cases = vec![
            GrpcTestCase {
                name: "Success".into(),
                method: method.into(),
                request: request.clone(),
                request_grpc: grpc_request.clone(),
                response_grpc: GetReceiptCredentialResponse {
                    response: Some(get_receipt_credential_response::Response::Success(
                        GetReceiptCredentialResult {
                            receipt_credential_response: zkgroup::serialize(&valid_response),
                        },
                    )),
                },
                response: GetReceiptCredentialOut::Success(
                    server_params
                        .receive_receipt_credential(&ctx, &valid_response)
                        .expect("can receive"),
                ),
            },
            GrpcTestCase {
                name: "Misaligned expiration".into(),
                method: method.into(),
                request: request.clone(),
                request_grpc: grpc_request.clone(),
                response_grpc: GetReceiptCredentialResponse {
                    response: Some(get_receipt_credential_response::Response::Success(
                        GetReceiptCredentialResult {
                            receipt_credential_response: zkgroup::serialize(&issue_receipt(
                                200,
                                zkgroup::Timestamp::from_epoch_seconds(
                                    day_align(now_seconds + 3 * SECONDS_PER_DAY) + 1,
                                ),
                            )),
                        },
                    )),
                },
                response: GetReceiptCredentialOut::UnexpectedError {
                    contains: UNEXPECTED_CANT_RECV.into(),
                },
            },
            GrpcTestCase {
                name: "Already expired".into(),
                method: method.into(),
                request: request.clone(),
                request_grpc: grpc_request.clone(),
                response_grpc: GetReceiptCredentialResponse {
                    response: Some(get_receipt_credential_response::Response::Success(
                        GetReceiptCredentialResult {
                            receipt_credential_response: zkgroup::serialize(&issue_receipt(
                                200,
                                zkgroup::Timestamp::from_epoch_seconds(day_align(
                                    now_seconds - 3 * SECONDS_PER_DAY,
                                )),
                            )),
                        },
                    )),
                },
                response: GetReceiptCredentialOut::UnexpectedError {
                    contains: UNEXPECTED_EXPIRATION_OUT_OF_RANGE.into(),
                },
            },
            GrpcTestCase {
                name: "Expires too far in the future".into(),
                method: method.into(),
                request: request.clone(),
                request_grpc: grpc_request.clone(),
                response_grpc: GetReceiptCredentialResponse {
                    response: Some(get_receipt_credential_response::Response::Success(
                        GetReceiptCredentialResult {
                            receipt_credential_response: zkgroup::serialize(&issue_receipt(
                                200,
                                zkgroup::Timestamp::from_epoch_seconds(day_align(
                                    now_seconds + 91 * SECONDS_PER_DAY,
                                )),
                            )),
                        },
                    )),
                },
                response: GetReceiptCredentialOut::UnexpectedError {
                    contains: UNEXPECTED_EXPIRATION_OUT_OF_RANGE.into(),
                },
            },
            GrpcTestCase {
                name: "Just barely expired but still accepted".into(),
                method: method.into(),
                request: request.clone(),
                request_grpc: grpc_request.clone(),
                response_grpc: GetReceiptCredentialResponse {
                    response: Some(get_receipt_credential_response::Response::Success(
                        GetReceiptCredentialResult {
                            receipt_credential_response: zkgroup::serialize(
                                &just_barely_expired_response,
                            ),
                        },
                    )),
                },
                response: GetReceiptCredentialOut::Success(
                    server_params
                        .receive_receipt_credential(&ctx, &just_barely_expired_response)
                        .expect("can receive"),
                ),
            },
        ];
        // Simple Error cases
        for (nice_error, grpc_error) in [
            (
                ReceiptCredentialError::NoPaidInvoice,
                get_receipt_credential_response::Response::NoPaidInvoice(FailedPrecondition {
                    description: "error".into(),
                }),
            ),
            (
                ReceiptCredentialError::SubscriberNotFound,
                get_receipt_credential_response::Response::SubscriberNotFound(NotFound {}),
            ),
            (
                ReceiptCredentialError::ReceiptAlreadyIssued,
                get_receipt_credential_response::Response::AlreadyRedeemed(FailedPrecondition {
                    description: "error".into(),
                }),
            ),
        ] {
            test_cases.push(GrpcTestCase {
                name: format!("Error {nice_error:?}"),
                method: method.to_string(),
                request: request.clone(),
                request_grpc: grpc_request.clone(),
                response_grpc: GetReceiptCredentialResponse {
                    response: Some(grpc_error),
                },
                response: GetReceiptCredentialOut::ExplicitError(nice_error),
            });
        }
        // Payment Required error cases
        test_cases.push(GrpcTestCase {
            name: "Payment Required: None".into(),
            method: method.to_string(),
            request: request.clone(),
            request_grpc: grpc_request.clone(),
            response_grpc: GetReceiptCredentialResponse {
                response: Some(get_receipt_credential_response::Response::PaymentRequired(
                    libsignal_net_grpc::proto::chat::purchase::PaymentRequired {
                        charge_failure: None,
                    },
                )),
            },
            response: GetReceiptCredentialOut::ExplicitError(
                ReceiptCredentialError::PaymentRequired {
                    charge_failure: None,
                },
            ),
        });
        test_cases.push(GrpcTestCase {
            name: "Payment Required: None fields".into(),
            method: method.to_string(),
            request: request.clone(),
            request_grpc: grpc_request.clone(),
            response_grpc: GetReceiptCredentialResponse {
                response: Some(get_receipt_credential_response::Response::PaymentRequired(
                    libsignal_net_grpc::proto::chat::purchase::PaymentRequired {
                        charge_failure: Some(GrpcChargeFailure {
                            processor: GrpcPaymentProvider::GooglePlayBilling.into(),
                            code: "code".into(),
                            message: "message".into(),
                            outcome_network_status: None,
                            outcome_reason: None,
                            outcome_type: None,
                        }),
                    },
                )),
            },
            response: GetReceiptCredentialOut::ExplicitError(
                ReceiptCredentialError::PaymentRequired {
                    charge_failure: Some(Box::new(ChargeFailure {
                        processor: PaymentProvider::GooglePlayBilling,
                        code: "code".into(),
                        message: "message".into(),
                        outcome_network_status: None,
                        outcome_reason: None,
                        outcome_type: None,
                    })),
                },
            ),
        });
        test_cases.push(GrpcTestCase {
            name: "Payment Required: Some fields".into(),
            method: method.to_string(),
            request: request.clone(),
            request_grpc: grpc_request.clone(),
            response_grpc: GetReceiptCredentialResponse {
                response: Some(get_receipt_credential_response::Response::PaymentRequired(
                    libsignal_net_grpc::proto::chat::purchase::PaymentRequired {
                        charge_failure: Some(GrpcChargeFailure {
                            processor: GrpcPaymentProvider::GooglePlayBilling.into(),
                            code: "code".into(),
                            message: "message".into(),
                            outcome_network_status: Some("ons".into()),
                            outcome_reason: Some("or".into()),
                            outcome_type: Some("ot".into()),
                        }),
                    },
                )),
            },
            response: GetReceiptCredentialOut::ExplicitError(
                ReceiptCredentialError::PaymentRequired {
                    charge_failure: Some(Box::new(ChargeFailure {
                        processor: PaymentProvider::GooglePlayBilling,
                        code: "code".into(),
                        message: "message".into(),
                        outcome_network_status: Some("ons".into()),
                        outcome_reason: Some("or".into()),
                        outcome_type: Some("ot".into()),
                    })),
                },
            ),
        });
        test_cases
    }
}

#[cfg(test)]
mod tests {
    use super::test_cases::*;
    use super::*;
    use crate::grpc::testutil::run_tests;

    #[test]
    fn test_get_receipt_credential() {
        run_tests(
            get_receipt_credential_test_cases(),
            |chat: Unauth<_>,
             GetReceiptCredentialArgs {
                 subscriber_id,
                 receipt_credential_request_context,
                 server_params,
             }| async move {
                chat.get_subscription_receipt_credential(
                    subscriber_id,
                    &receipt_credential_request_context,
                    &server_params,
                )
                .await
            },
            |out, result| match out {
                GetReceiptCredentialOut::Success(receipt) => {
                    assert_eq!(
                        zkgroup::serialize(&result.expect("success")),
                        zkgroup::serialize(&receipt)
                    );
                }
                // assert_matches!() would require that ReceiptCredential impl Debug
                GetReceiptCredentialOut::UnexpectedError { contains } => assert!(
                    matches!(&result, Err(RequestError::Unexpected { log_safe }) if log_safe.contains(&contains)),
                    "Got {:?}. Expected the unexpected with: {contains:?}",
                    result.err()
                ),
                GetReceiptCredentialOut::ExplicitError(explicit_error) => {
                    assert!(matches!(result, Err(RequestError::Other(e)) if e == explicit_error))
                }
            },
        );
    }
}
