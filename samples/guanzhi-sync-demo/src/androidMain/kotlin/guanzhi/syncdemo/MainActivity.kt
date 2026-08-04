package guanzhi.syncdemo

import android.app.Activity
import android.os.Bundle
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import com.kmosync.KmoSync
import com.kmosync.KmoSyncConfig
import kotlinx.coroutines.runBlocking
import java.io.File

/**
 * 观止 × Book Sync 双端同步演示（Android）。
 *
 * 单台手机上同时创建两个 KmoSync 实例模拟「手机 phone-1」和「平板 pad-1」，
 * 共用应用外部存储目录作为远端，跑一遍进度 / 书签 / 划线 / 笔记 / 删除 /
 * 复活 的完整同步场景并逐条显示 PASS/FAIL。
 *
 * 安装：gradle :samples:guanzhi-sync-demo:assembleDebug
 * 产物：samples/guanzhi-sync-demo/build/outputs/apk/debug/app-debug.apk
 */
class MainActivity : Activity() {

    private lateinit var logView: TextView
    private lateinit var runButton: Button

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(buildLayout())
    }

    private fun buildLayout() = LinearLayout(this).apply {
        orientation = LinearLayout.VERTICAL
        setPadding(32, 32, 32, 32)

        val title = TextView(this@MainActivity).apply {
            text = "观止 × Book Sync 双端同步演示"
            textSize = 20f
        }
        addView(title)

        runButton = Button(this@MainActivity).apply {
            text = "运行双端同步演示"
            setOnClickListener { runDemo() }
        }
        addView(runButton)

        logView = TextView(this@MainActivity).apply {
            textSize = 12f
        }
        val scroll = ScrollView(this@MainActivity).apply {
            addView(logView)
        }
        addView(scroll, LinearLayout.LayoutParams(
            LinearLayout.LayoutParams.MATCH_PARENT,
            0,
            1f,
        ))
    }

    private fun runDemo() {
        runButton.isEnabled = false
        logView.text = ""
        Thread {
            try {
                runBlocking {
                    runTwoDeviceScenario()
                }
            } catch (t: Throwable) {
                appendLog("异常: $t")
            } finally {
                runOnUiThread { runButton.isEnabled = true }
            }
        }.start()
    }

    private suspend fun runTwoDeviceScenario() {
        val remote = File(getExternalFilesDir(null), "kmo-sync-remote").apply { mkdirs() }
        val phoneCache = File(filesDir, "kmo-cache-phone").apply { mkdirs() }
        val padCache = File(filesDir, "kmo-cache-pad").apply { mkdirs() }
        appendLog("远端目录: $remote")

        val phone = createDevice("phone-1", phoneCache, remote)
        val pad = createDevice("pad-1", padCache, remote)
        var passed = 0
        var failed = 0
        try {
            val scenario = SyncScenario { name, ok ->
                if (ok) passed++ else failed++
                appendLog("${if (ok) "PASS" else "FAIL"}  $name")
            }
            scenario.run(phone, pad) { sync ->
                when (sync) {
                    phone -> "phone-1"
                    pad -> "pad-1"
                    else -> "device"
                }
            }
        } finally {
            phone.close()
            pad.close()
        }
        appendLog("")
        appendLog("=== 结果: $passed 通过, $failed 失败 ===")
        if (failed == 0) appendLog("远端协议对象: $remote")
    }

    private fun createDevice(deviceId: String, cache: File, remote: File): KmoSync =
        KmoSync(
            KmoSyncConfig(
                storageConfigJson = """{"type":"file","root_dir":"${remote.absolutePath}"}""",
                encryptionConfigJson = """{"type":"none"}""",
                deviceId = deviceId,
                localCacheDir = cache.absolutePath,
            ),
        )

    private fun appendLog(line: String) {
        runOnUiThread {
            logView.append(line + "\n")
        }
    }
}
