package sh.cosmos.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.cosmos.companion.net.ChatIndicator
import sh.cosmos.companion.net.SessionStatus
import sh.cosmos.companion.net.SessionSummary

class SessionLogicTest {
    private fun session(
        status: SessionStatus,
        statusUpdatedAt: Long?,
        lastMessageAt: Long? = null,
        lastSeenAt: Long? = null,
    ) = SessionSummary(
        chatId = "c", title = "t", harness = "mock", model = "m", reasoning = null,
        branch = null, cwd = null, spaceId = null, deviceId = "d", archived = false,
        lastMessagePreview = null, lastMessageAt = lastMessageAt, createdAt = 0,
        lastSeenAt = lastSeenAt, status = status, statusUpdatedAt = statusUpdatedAt,
    )

    @Test fun workingIsWorkingWhenFresh() {
        val now = 100_000L
        val s = session(SessionStatus.working, statusUpdatedAt = now - 1_000)
        assertEquals(ChatIndicator.working, s.indicator(now))
    }

    @Test fun staleWorkingFallsBackToUnseenOrIdle() {
        val now = 100_000L
        // stale (>45s) working with unseen message => completed; else idle
        val unseen = session(SessionStatus.working, statusUpdatedAt = now - 60_000, lastMessageAt = now, lastSeenAt = 0)
        assertEquals(ChatIndicator.completed, unseen.indicator(now))
        val seen = session(SessionStatus.working, statusUpdatedAt = now - 60_000, lastMessageAt = 10, lastSeenAt = 20)
        assertEquals(ChatIndicator.idle, seen.indicator(now))
    }

    @Test fun erroredOnlyWhenUnseen() {
        val now = 100_000L
        val unseen = session(SessionStatus.errored, statusUpdatedAt = now, lastMessageAt = now, lastSeenAt = null)
        assertEquals(ChatIndicator.errored, unseen.indicator(now))
        val seen = session(SessionStatus.errored, statusUpdatedAt = now, lastMessageAt = 10, lastSeenAt = 20)
        assertEquals(ChatIndicator.idle, seen.indicator(now))
    }

    @Test fun unseenComputation() {
        assertTrue(session(SessionStatus.idle, null, lastMessageAt = 5, lastSeenAt = null).unseen)
        assertFalse(session(SessionStatus.idle, null, lastMessageAt = 5, lastSeenAt = 9).unseen)
        assertTrue(session(SessionStatus.idle, null, lastMessageAt = 9, lastSeenAt = 5).unseen)
    }
}
