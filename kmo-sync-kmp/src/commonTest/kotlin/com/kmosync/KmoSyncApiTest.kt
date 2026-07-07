package com.kmosync

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertFailsWith

class KmoSyncApiTest {
    @Test
    fun wireValuesMatchCAbi() {
        assertEquals(0, SyncMode.Bidirectional.wireValue)
        assertEquals(1, SyncMode.PushOnly.wireValue)
        assertEquals(2, SyncMode.PullOnly.wireValue)

        assertEquals(0, NetworkType.Wifi.wireValue)
        assertEquals(1, NetworkType.Cellular.wireValue)
        assertEquals(2, NetworkType.Unknown.wireValue)

        assertEquals(1, ErrorCode.Network.code)
        assertEquals(5, ErrorCode.InvalidArg.code)
    }

    @Test
    fun eventTypesMapUnknownValues() {
        assertEquals(SyncEventType.SyncStart, SyncEventType.fromWireValue(1))
        assertEquals(SyncEventType.ClockDriftWarning, SyncEventType.fromWireValue(12))
        assertEquals(SyncEventType.Unknown, SyncEventType.fromWireValue(99))
    }

    @Test
    fun failureResultCarriesCodeAndMessage() {
        val result: SyncResult = SyncResult.Failure(5, "invalid argument")
        assertIs<SyncResult.Failure>(result)
        assertEquals(5, result.code)
        assertEquals("invalid argument", result.message)
    }

    @Test
    fun syncIntervalPresetsUseExpectedSeconds() {
        assertEquals(
            listOf(10L, 20L, 30L, 60L, 120L, 180L, 300L),
            SyncIntervalOption.presets.map { it.seconds },
        )
        assertEquals(10_000L, SyncIntervalOption.TenSeconds.milliseconds)
        assertEquals(300_000L, SyncIntervalOption.FiveMinutes.milliseconds)
    }

    @Test
    fun customSyncIntervalUsesFiveSecondMultiplier() {
        assertEquals(5L, SyncIntervalOption.custom(1).seconds)
        assertEquals(35L, SyncIntervalOption.custom(7).seconds)
        assertEquals(35_000L, SyncIntervalOption.custom(7).milliseconds)
        assertFailsWith<IllegalArgumentException> {
            SyncIntervalOption.custom(0)
        }
    }
}
