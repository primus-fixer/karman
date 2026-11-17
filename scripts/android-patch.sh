#!/usr/bin/env bash
# Своя донастройка Android-проекта «Кармана» — после общей из kit (название, иконки, разрешения сети,
# запрет копии Android и targetSdk делает `kit/scripts/android-patch.sh`): пустое превью в недавних.
#
#   bash scripts/android-patch.sh <gen/android>
set -euo pipefail

PROJECT="${1:?путь к gen/android}"
MAIN="$PROJECT/app/src/main"
MANIFEST="$MAIN/AndroidManifest.xml"

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

echo "Android-проект донастроен (своё у «Кармана»):"
grep -E "uses-permission|allowBackup|dataExtractionRules|targetSdk|minSdk" -r "$MANIFEST" "$PROJECT/app/build.gradle.kts" 2>/dev/null || true
if [ -n "${ACTIVITY:-}" ]; then cat "$ACTIVITY"; fi
