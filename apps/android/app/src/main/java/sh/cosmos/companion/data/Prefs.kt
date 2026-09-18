package sh.cosmos.companion.data

import android.content.Context
import java.util.UUID
import sh.cosmos.companion.net.Connection

/** Small SharedPreferences-backed store for the last connection + a stable
 *  device id (the phone is a peer device; its id persists across launches). */
class Prefs(context: Context) {
    private val sp = context.getSharedPreferences("cosmos", Context.MODE_PRIVATE)

    val deviceId: String
        get() = sp.getString("deviceId", null) ?: UUID.randomUUID().toString().also {
            sp.edit().putString("deviceId", it).apply()
        }

    fun saveConnection(c: Connection) {
        sp.edit()
            .putString("edgeUrl", c.edgeUrl)
            .putString("userId", c.userId)
            .putString("orgId", c.orgId)
            .putString("token", c.token)
            .putString("deviceName", c.deviceName)
            .putString("mode", c.mode)
            .apply()
    }

    fun loadConnection(): Connection? {
        val edgeUrl = sp.getString("edgeUrl", null) ?: return null
        val userId = sp.getString("userId", null) ?: return null
        val orgId = sp.getString("orgId", null) ?: return null
        val token = sp.getString("token", null) ?: return null
        return Connection(
            edgeUrl = edgeUrl,
            userId = userId,
            orgId = orgId,
            token = token,
            deviceId = deviceId,
            deviceName = sp.getString("deviceName", "Cosmos Android") ?: "Cosmos Android",
            mode = sp.getString("mode", "dev") ?: "dev",
        )
    }

    fun clear() {
        sp.edit()
            .remove("edgeUrl").remove("userId").remove("orgId")
            .remove("token").remove("deviceName").remove("mode")
            .apply()
    }
}
