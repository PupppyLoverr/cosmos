package sh.cosmos.companion

import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.cosmos.companion.net.SessionTail
import sh.cosmos.companion.net.toTranscriptItem

class TailParseTest {
    private val json = Json { ignoreUnknownKeys = true }

    @Test fun parsesHostPublishedTail() {
        val raw = """
            {
              "chatId": "c1",
              "schemaVersion": 1,
              "totalMessages": 2,
              "updatedAt": 1000,
              "messages": [
                {"id":"m1","role":"user","createdAt":1,"parts":[{"id":"p1","kind":"text","text":"hello"}]},
                {"id":"m2","role":"assistant","createdAt":2,"status":"complete","parts":[{"id":"p2","kind":"text","text":"world"}]}
              ]
            }
        """.trimIndent()
        val tail = json.decodeFromString(SessionTail.serializer(), raw)
        assertEquals("c1", tail.chatId)
        assertEquals(2, tail.messages.size)
        val items = tail.messages.mapNotNull { it.toTranscriptItem() }
        assertEquals(2, items.size)
        assertEquals("user", items[0].role)
        assertEquals("hello", items[0].text)
        assertEquals("assistant", items[1].role)
        assertEquals("world", items[1].text)
    }

    @Test fun ignoresEmptyParts() {
        val raw = """{"messages":[{"id":"x","role":"assistant","createdAt":1,"parts":[{"id":"p","kind":"tool"}]}]}"""
        val tail = json.decodeFromString(SessionTail.serializer(), raw)
        assertTrue(tail.messages.mapNotNull { it.toTranscriptItem() }.isEmpty())
    }
}
