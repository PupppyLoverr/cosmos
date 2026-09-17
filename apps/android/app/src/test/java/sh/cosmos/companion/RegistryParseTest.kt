package sh.cosmos.companion

import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.cosmos.companion.net.Registry
import sh.cosmos.companion.net.RegistryRow
import sh.cosmos.companion.net.RowsResponse

/**
 * Parses a payload captured live from a running Cosmos edge
 * (GET /registry/org1/rows, AUTH_MODE=dev, seeded by a headless mock engine)
 * and asserts the companion derives the sessions/spaces/devices exactly as the
 * desktop registry does. This exercises the real wire protocol end-to-end on
 * the JVM without an emulator.
 */
class RegistryParseTest {
    private val json = Json { ignoreUnknownKeys = true }

    private fun loadStore(): MutableMap<String, MutableMap<String, RegistryRow>> {
        val text = javaClass.classLoader!!
            .getResourceAsStream("registry_rows_live.json")!!
            .readBytes().decodeToString()
        val resp = json.decodeFromString(RowsResponse.serializer(), text)
        val store = HashMap<String, MutableMap<String, RegistryRow>>()
        Registry.applyDelta(store, resp)
        return store
    }

    @Test fun parsesLiveEdgePayload() {
        val store = loadStore()
        val sessions = Registry.sessions(store)
        val spaces = Registry.spaces(store)
        val devices = Registry.devices(store)

        assertEquals("5 chats seeded", 5, sessions.size)
        assertEquals("3 spaces seeded", 3, spaces.size)
        assertTrue("at least one device", devices.isNotEmpty())

        val titles = sessions.map { it.title }.toSet()
        assertTrue(titles.contains("Native Cosmos Rust Rewrite"))
        assertTrue(titles.contains("Rebalance Player Stats Caps"))

        // Config + branch survive the round trip.
        val rewrite = sessions.first { it.title == "Native Cosmos Rust Rewrite" }
        assertEquals("mock", rewrite.harness)
        assertEquals("fable-5", rewrite.model)
        assertEquals("cosmos/main", rewrite.branch)
        assertNotNull(rewrite.deviceId)

        // Every session has a resolvable display indicator.
        val now = System.currentTimeMillis()
        sessions.forEach { assertNotNull(it.indicator(now)) }
    }
}
