package sh.cosmos.companion

import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test
import sh.cosmos.companion.net.RegistryBridge
import sh.cosmos.companion.net.SessionStatus
import sh.cosmos.companion.net.SessionSummary
import sh.cosmos.companion.net.TranscriptItem
import sh.cosmos.companion.net.SpaceInfo
import sh.cosmos.companion.net.DeviceInfo
import sh.cosmos.companion.theme.CosmosTheme
import sh.cosmos.companion.theme.ThemeMode
import sh.cosmos.companion.ui.ConnectScreen
import sh.cosmos.companion.ui.SessionScreen
import sh.cosmos.companion.ui.SessionsScreen
import sh.cosmos.companion.ui.SettingsScreen
import sh.cosmos.companion.vm.ConnState
import sh.cosmos.companion.vm.LinkState
import sh.cosmos.companion.vm.Workspace

class ScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_6.copy(nightMode = NightMode.NIGHT),
        maxPercentDifference = 100.0,
    )

    private val now = System.currentTimeMillis()

    private fun sample(): List<SessionSummary> = listOf(
        SessionSummary("c1", "Native Cosmos Rust Rewrite", "mock", "fable-5", "medium", "cosmos/main", "/tmp",
            "s1", "d1", false, "Synced to every device through the session room. Mock harness reporting in.",
            now - 60_000, now - 3_600_000, 0, SessionStatus.working, now - 1_000),
        SessionSummary("c2", "Rebalance Player Stats Caps", "claude-code", "sonnet", null, "cosmos/rebalance", "/tmp",
            "s2", "d1", false, "Applied the cap curve and re-ran the balance sim.", now - 7_200_000, now - 8_000_000,
            0, SessionStatus.idle, now - 7_200_000),
        SessionSummary("c3", "Craft Premium TCG Experience", "codex", "gpt-5", "high", "cosmos/craft-premium", "/tmp",
            "s3", "d1", false, "Awaiting your call on the foil treatment.", now - 300_000, now - 90_000_000,
            0, SessionStatus.awaitingInput, now - 5_000),
        SessionSummary("c4", "Initial Context Exploration", "cursor", "composer", null, "cosmos/initial", "/tmp",
            "s4", "d1", false, "Indexed the workspace and mapped the crates.", now - 500_000_000, now - 500_000_000,
            now, SessionStatus.idle, now - 500_000_000),
        SessionSummary("c5", "Soccer TCG Repo Creation", "amp", "default", null, "aether/main", "/tmp",
            "s5", "d1", false, "Repo scaffolded; CI green.", now - 900_000_000, now - 900_000_000,
            now, SessionStatus.errored, now - 900_000_000),
    )

    private fun workspace() = Workspace(
        sessions = sample(),
        spaces = listOf(SpaceInfo("s1", "/home/dev/github/cosmos", null, "d1")),
        devices = listOf(DeviceInfo("d1", "Desktop (Linux)", "linux", now)),
        presence = mapOf("d1" to now),
    )

    @Test fun connect() {
        paparazzi.snapshot {
            CosmosTheme(ThemeMode.Dark) {
                ConnectScreen(ConnState.Disconnected, "http://10.0.2.2:27640", "alice", "org1") { _, _, _ -> }
            }
        }
    }

    @Test fun sessions() {
        paparazzi.snapshot {
            CosmosTheme(ThemeMode.Dark) {
                SessionsScreen(workspace(), LinkState.Online, "", {}, {}, {}, {})
            }
        }
    }

    @Test fun sessionsLight() {
        paparazzi.snapshot {
            CosmosTheme(ThemeMode.Light) {
                SessionsScreen(workspace(), LinkState.Reconnecting, "cosmos", {}, {}, {}, {})
            }
        }
    }

    @Test fun session() {
        paparazzi.snapshot {
            CosmosTheme(ThemeMode.Dark) {
                SessionScreen(
                    sample()[0],
                    client = null,
                    bridge = RegistryBridge(),
                    seedItems = listOf(
                        TranscriptItem("u1", "user", "Stand up a native Android companion for Cosmos sessions.", now - 90_000),
                        TranscriptItem("a1", "assistant", "Synced to every device through the session room. Mock harness reporting in.", now - 60_000),
                    ),
                    onBack = {},
                )
            }
        }
    }

    @Test fun settings() {
        paparazzi.snapshot {
            CosmosTheme(ThemeMode.Dark) {
                SettingsScreen(null, LinkState.Online, ThemeMode.Dark, {}, {}, {}, {})
            }
        }
    }
}
