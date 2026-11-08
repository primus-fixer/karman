#!/usr/bin/env bash
# Донастройка Android-проекта, который создаёт `tauri android init`:
# русское название, иконки и разрешения для связи по Wi-Fi.
set -euo pipefail

PROJECT="${1:?путь к gen/android}"
ICONS="${2:-}"
MAIN="$PROJECT/app/src/main"
MANIFEST="$MAIN/AndroidManifest.xml"

# Название под иконкой.
for f in "$MAIN"/res/values/strings.xml; do
  [ -f "$f" ] || continue
  sed -i -E 's#(<string name="(app_name|main_activity_title)">)[^<]*(</string>)#\1Карман\3#' "$f"
done

# Иконки, которые сделал `tauri icon`.
if [ -n "$ICONS" ] && [ -d "$ICONS" ]; then
  cp -r "$ICONS"/. "$MAIN/res/"
fi

# Разрешения: состояние сети и Wi-Fi, мультикаст для поиска устройств в сети.
# INTERNET уже есть в шаблоне, CAMERA добавляет модуль сканера QR-кодов.
add_permission() {
  local name="$1"
  grep -q "android:name=\"$name\"" "$MANIFEST" && return 0
  sed -i "s#<application#<uses-permission android:name=\"$name\" />\n    <application#" "$MANIFEST"
}
add_permission android.permission.ACCESS_NETWORK_STATE
add_permission android.permission.ACCESS_WIFI_STATE
add_permission android.permission.CHANGE_WIFI_MULTICAST_STATE
add_permission android.permission.ACCESS_LOCAL_NETWORK

# Резервное копирование Android (Google Диск и перенос на новый телефон) выключено:
# база зашифрована ключом из Keystore, который не переносится, — на другом телефоне копия
# всё равно не откроется. Для переноса — свои резервные копии и связь по Wi-Fi.
if ! grep -q 'android:allowBackup' "$MANIFEST"; then
  sed -i 's#<application#<application\n        android:allowBackup="false"\n        android:dataExtractionRules="@xml/data_extraction_rules"#' "$MANIFEST"
fi
mkdir -p "$MAIN/res/xml"
cat > "$MAIN/res/xml/data_extraction_rules.xml" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<data-extraction-rules>
    <cloud-backup>
        <exclude domain="root" path="." />
        <exclude domain="file" path="." />
        <exclude domain="database" path="." />
        <exclude domain="sharedpref" path="." />
        <exclude domain="external" path="." />
    </cloud-backup>
    <device-transfer>
        <exclude domain="root" path="." />
        <exclude domain="file" path="." />
        <exclude domain="database" path="." />
        <exclude domain="sharedpref" path="." />
        <exclude domain="external" path="." />
    </device-transfer>
</data-extraction-rules>
XML

# Превью «Кармана» в списке недавних приложений — пустое; снимки экрана делать можно.
# Android 13+: setRecentsScreenshotEnabled(false). Раньше: на время ухода в фон — FLAG_SECURE.
ACTIVITY=$(find "$MAIN" -name MainActivity.kt | head -1)
if [ -n "$ACTIVITY" ] && ! grep -q setRecentsScreenshotEnabled "$ACTIVITY"; then
  PACKAGE=$(grep -m1 '^package ' "$ACTIVITY")
  EDGE=""
  grep -q enableEdgeToEdge "$ACTIVITY" && EDGE="    enableEdgeToEdge()"
  {
    echo "$PACKAGE"
    echo
    echo "import android.os.Build"
    echo "import android.os.Bundle"
    echo "import android.view.WindowManager"
    [ -n "$EDGE" ] && echo "import androidx.activity.enableEdgeToEdge"
    cat <<'KT'

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
KT
    [ -n "$EDGE" ] && echo "$EDGE"
    cat <<'KT'
    super.onCreate(savedInstanceState)
    if (Build.VERSION.SDK_INT >= 33) setRecentsScreenshotEnabled(false)
  }

  override fun onPause() {
    if (Build.VERSION.SDK_INT < 33) window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
    super.onPause()
  }

  override fun onResume() {
    super.onResume()
    if (Build.VERSION.SDK_INT < 33) window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
  }
}
KT
  } > "$ACTIVITY.new"
  mv "$ACTIVITY.new" "$ACTIVITY"
fi

# Пока targetSdk 36. Приложениям с targetSdk 37 на Android 17 нужно отдельно
# запрашивать доступ к локальной сети — перейдём, когда добавим этот запрос.
GRADLE="$PROJECT/app/build.gradle.kts"
[ -f "$GRADLE" ] && sed -i -E 's/targetSdk = [0-9]+/targetSdk = 36/' "$GRADLE"

echo "Android-проект донастроен:"
grep -E "uses-permission|allowBackup|dataExtractionRules|targetSdk|minSdk" -r "$MANIFEST" "$PROJECT/app/build.gradle.kts" 2>/dev/null || true
if [ -n "${ACTIVITY:-}" ]; then cat "$ACTIVITY"; fi
