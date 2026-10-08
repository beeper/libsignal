//
// Copyright 2026 Signal Messenger, LLC.
// SPDX-License-Identifier: AGPL-3.0-only
//

package org.signal.libsignal.net

import org.signal.libsignal.internal.CalledFromNative
import java.io.IOException

public sealed class PaymentProvider {
  public data object GooglePlayBilling : PaymentProvider()

  public data object AppleAppStore : PaymentProvider()

  public data object Stripe : PaymentProvider()

  public data object Braintree : PaymentProvider()
}

public data class ChargeFailure(
  public val processor: PaymentProvider,
  public val code: String,
  public val message: String,
  public val outcomeNetworkStatus: String?,
  public val outcomeReason: String?,
  public val outcomeType: String?,
)

public sealed class ReceiptCredentialException(
  message: String,
) : IOException(message),
  BadRequestError {
  /**
   * The purchase is still pending with the payment provider. The client may retry later.
   *
   * See the specific request docs for more information.
   */
  public class PaymentStillProcessing : ReceiptCredentialException {
    @CalledFromNative
    public constructor(message: String) : super(message)
  }

  /**
   * The purchase did not complete successfully.
   */
  public class PaymentRequired : ReceiptCredentialException {
    public val chargeFailure: ChargeFailure?

    @CalledFromNative
    public constructor(message: String, chargeFailure: ChargeFailure?) : super(message) {
      this.chargeFailure = chargeFailure
    }
  }

  /**
   * The payment provider has no purchase with the provided identifier.
   *
   * See the specific request docs for more information.
   */
  public class PaymentNotFound : ReceiptCredentialException {
    @CalledFromNative
    public constructor(message: String) : super(message)
  }

  /**
   * The purchase was already redeemed for a receipt credential, but with a different receipt
   * credential request.
   */
  public class ReceiptAlreadyIssued : ReceiptCredentialException {
    @CalledFromNative
    public constructor(message: String) : super(message)
  }
}
