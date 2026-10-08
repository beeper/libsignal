//
// Copyright 2026 Signal Messenger, LLC.
// SPDX-License-Identifier: AGPL-3.0-only
//

import type {
  ReceiptCredentialErrorPaymentNotFound,
  ReceiptCredentialErrorPaymentRequired,
  ReceiptCredentialErrorPaymentStillProcessing,
  ReceiptCredentialErrorReceiptAlreadyIssued,
} from '../../Errors.js';

export type PaymentProvider =
  | 'googlePlayBilling'
  | 'appleAppStore'
  | 'stripe'
  | 'braintree';

export type ReceiptCredentialError =
  | ReceiptCredentialErrorPaymentNotFound
  | ReceiptCredentialErrorPaymentRequired
  | ReceiptCredentialErrorPaymentStillProcessing
  | ReceiptCredentialErrorReceiptAlreadyIssued;
