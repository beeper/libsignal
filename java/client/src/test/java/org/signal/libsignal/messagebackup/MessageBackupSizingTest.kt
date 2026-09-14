//
// Copyright 2026 Signal Messenger, LLC.
// SPDX-License-Identifier: AGPL-3.0-only
//

package org.signal.libsignal.messagebackup

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class MessageBackupSizingTest {
  companion object {
    private const val MIN_INTERVAL_BYTES = 8L * 1024
    private const val MINIMUM_BACKUP_BYTES = 64L * 1024
  }

  @Test
  fun flushIntervalTestValues() {
    assertEquals(MIN_INTERVAL_BYTES, MessageBackupSizing.flushInterval(0L))
    assertEquals(9_459L, MessageBackupSizing.flushInterval(65_536L))
    assertEquals(37_837L, MessageBackupSizing.flushInterval(1_048_576L))
    assertEquals(151_348L, MessageBackupSizing.flushInterval(16_777_216L))
    assertEquals(522_557L, MessageBackupSizing.flushInterval(200_000_000L))

    assertEquals(26_754L, MessageBackupSizing.flushInterval(0L, 1_048_576L))
    assertEquals(107_019L, MessageBackupSizing.flushInterval(0L, 16_777_216L))
  }

  @Test
  fun estimateHoldsAFixedIntervalUntilTheBackupCrossesIt() {
    val estimate = 16_777_216L
    assertEquals(
      MessageBackupSizing.flushInterval(0L, estimate),
      MessageBackupSizing.flushInterval(estimate / 2, estimate),
    )
    assertTrue(
      MessageBackupSizing.flushInterval(0L, estimate) < MessageBackupSizing.flushInterval(estimate),
    )

    // Past the estimate, the interval is the one that suits a backup ending here.
    assertEquals(
      MessageBackupSizing.flushInterval(0L, 4 * estimate),
      MessageBackupSizing.flushInterval(4 * estimate, estimate),
    )
  }

  @Test
  fun estimateMustBePositive() {
    assertThrows(IllegalArgumentException::class.java) {
      MessageBackupSizing.flushInterval(1_000L, 0L)
    }
  }

  @Test
  fun smallBackupsAllComeOutTheSameSize() {
    val compressedLength = 1_000L
    repeat(100) {
      assertEquals(
        MINIMUM_BACKUP_BYTES,
        compressedLength + MessageBackupSizing.paddingSize(MIN_INTERVAL_BYTES, compressedLength),
      )
    }
  }
}
