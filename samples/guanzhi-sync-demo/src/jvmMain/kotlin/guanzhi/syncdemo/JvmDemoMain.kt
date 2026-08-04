package guanzhi.syncdemo

import com.kmosync.KmoSync
import com.kmosync.KmoSyncConfig
import kotlinx.coroutines.runBlocking
import java.nio.file.Files
import java.nio.file.Path

/**
 * JVM 命令行版双端同步演示：
 *   gradle :samples:guanzhi-sync-demo:runDemo
 *
 * 可选环境变量 KMO_DEMO_REMOTE_DIR 指定共享远端目录（默认临时目录）。
 */
fun main() = runBlocking {
    val remoteDir: Path =
        System.getenv("KMO_DEMO_REMOTE_DIR")?.let(Path::of)
            ?: Files.createTempDirectory("guanzhi-sync-remote")
    val cachePhone = Files.createTempDirectory("guanzhi-phone-cache")
    val cachePad = Files.createTempDirectory("guanzhi-pad-cache")

    println("共享远端目录: $remoteDir")
    println()

    val phone = createDevice("phone-1", cachePhone, remoteDir)
    val pad = createDevice("pad-1", cachePad, remoteDir)

    val checks = mutableListOf<Pair<String, Boolean>>()
    val scenario = SyncScenario { name, ok -> checks.add(name to ok) }
    var passed = 0
    try {
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

    println()
    println("=== 观止 x Book Sync 同步演示结果 ===")
    checks.forEach { (name, ok) ->
        println("${if (ok) "PASS" else "FAIL"}  $name")
        if (ok) passed++
    }
    println("$passed/${checks.size} 通过")
    if (checks.any { !it.second }) {
        kotlin.system.exitProcess(1)
    }
}

private fun createDevice(deviceId: String, cache: Path, remote: Path): KmoSync =
    KmoSync(
        KmoSyncConfig(
            storageConfigJson =
                """{"type":"file","root_dir":"${remote.toString().replace("\\", "/")}"}""",
            encryptionConfigJson = """{"type":"none"}""",
            deviceId = deviceId,
            localCacheDir = cache.toString(),
        ),
    )
