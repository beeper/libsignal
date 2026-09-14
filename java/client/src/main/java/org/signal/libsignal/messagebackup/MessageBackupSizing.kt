//
// Copyright 2026 Signal Messenger, LLC.
// SPDX-License-Identifier: AGPL-3.0-only
//

package org.signal.libsignal.messagebackup

import org.signal.libsignal.internal.Native

/**
 * Sizing decisions for a compressed backup stream.
 *
 * The caller owns the compressor and does the flushing itself; this object provides the arithmetic.
 */
public object MessageBackupSizing {
  /**
   * Uncompressed bytes to write before ending the current DEFLATE block, for a writer that does not
   * know how large the backup will be.
   *
   * The interval grows with position, which is what lets a reasonable value be chosen without
   * knowing the final size of the backup in advance.
   *
   * @param uncompressedLength uncompressed bytes written since the chat item region began.
   */
  @JvmStatic
  public fun flushInterval(uncompressedLength: Long): Long =
    Native.MessageBackupSizing_FlushInterval(uncompressedLength, 0)

  /**
   * Uncompressed bytes to write before ending the current DEFLATE block, for a writer that can
   * estimate the total uncompressed length of the backup. The estimate affects compression
   * efficiency, but has no impact on security. The estimate does not need to be accurate.
   *
   * The *uncompressed* length of the previous backup is a good estimate.
   *
   * @param uncompressedLength uncompressed bytes written since the chat item region began.
   * @param estimatedTotalUncompressedLength estimated total uncompressed length of the backup.
   * @throws IllegalArgumentException if the estimate is not positive.
   */
  @JvmStatic
  public fun flushInterval(
    uncompressedLength: Long,
    estimatedTotalUncompressedLength: Long,
  ): Long {
    require(estimatedTotalUncompressedLength > 0) { "estimate must be positive" }
    return Native.MessageBackupSizing_FlushInterval(
      uncompressedLength,
      estimatedTotalUncompressedLength,
    )
  }

  /**
   * Number of zero bytes to append to a finished backup stream.
   *
   * @param maxIntervalBytes the largest DEFLATE block the writer *actually produced* in the chat
   *   item region, in uncompressed bytes.
   * @param compressedLength length of the compressed stream, before padding.
   */
  @JvmStatic
  public fun paddingSize(
    maxIntervalBytes: Long,
    compressedLength: Long,
  ): Long = Native.MessageBackupSizing_PaddingSize(maxIntervalBytes, compressedLength)
}
