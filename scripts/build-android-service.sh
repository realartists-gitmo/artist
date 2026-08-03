#!/usr/bin/env bash
# Build the accessibility bridge APK.
#
# Deliberately not Gradle. The service is one Java file and two resources, and
# the Android Gradle Plugin would bring a JDK toolchain, a daemon, a lockfile
# and a plugin resolution step into a repository that has kept itself to one
# toolchain on purpose. What Gradle actually does for a project this size is
# call four tools in order, so this calls them in order:
#
#   aapt2 compile   resources    -> flat files
#   aapt2 link      manifest+res -> an APK with no code in it
#   javac           .java        -> .class
#   d8              .class       -> classes.dex
#   zip             dex into the APK
#   zipalign        align it
#   apksigner       sign it with a local debug key
#
# Requires the Android SDK's build-tools and one platform (for android.jar).
# Nothing else, and no network access.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
project="$here/crates/artist-computer/android-service"
out="${ARTIST_ANDROID_BUILD_DIR:-$here/target/android-service}"

fail() { printf 'build-android-service: %s\n' "$1" >&2; exit 1; }

# --- locate the SDK ---------------------------------------------------------
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [ -z "$sdk" ]; then
    for candidate in /opt/android-sdk "$HOME/Android/Sdk" "$HOME/.local/share/android-sdk"; do
        [ -d "$candidate" ] && sdk="$candidate" && break
    done
fi
[ -n "$sdk" ] || fail "no Android SDK found. Set ANDROID_HOME, or install one:
  Arch:   pacman -S android-sdk-build-tools android-sdk-platform-tools
          (and one platform, e.g. from the AUR: android-platform)
  Other:  https://developer.android.com/studio#command-tools"

# The newest build-tools and the newest platform, because neither is pinned by
# anything here and a stale one is a confusing failure rather than a safe one.
build_tools="$(ls -1d "$sdk"/build-tools/* 2>/dev/null | sort -V | tail -1 || true)"
[ -n "$build_tools" ] || fail "no build-tools under $sdk/build-tools"
android_jar="$(ls -1 "$sdk"/platforms/*/android.jar 2>/dev/null | sort -V | tail -1 || true)"
[ -n "$android_jar" ] || fail "no android.jar under $sdk/platforms — install a platform"

aapt2="$build_tools/aapt2"
d8="$build_tools/d8"
zipalign="$build_tools/zipalign"
apksigner="$build_tools/apksigner"
for tool in "$aapt2" "$d8" "$zipalign" "$apksigner"; do
    [ -x "$tool" ] || fail "missing $tool"
done
# A JDK, found rather than demanded. Distributions routinely install a *JRE* as
# the default java, so `javac` is absent from PATH on machines that have a
# perfectly good JDK sitting in /usr/lib/jvm. Insisting on PATH would send the
# reader off to install something they already have — and changing the system
# default to fix a build script is a rude thing for a build script to require.
javac=""
if [ -n "${JAVA_HOME:-}" ] && [ -x "$JAVA_HOME/bin/javac" ]; then
    javac="$JAVA_HOME/bin/javac"
elif command -v javac >/dev/null; then
    javac="$(command -v javac)"
else
    # Newest first, so a machine with several JDKs uses the most capable.
    javac="$(ls -1 /usr/lib/jvm/*/bin/javac 2>/dev/null | sort -V | tail -1 || true)"
fi
[ -n "$javac" ] && [ -x "$javac" ] || fail "no javac found. Install a JDK (17 or newer):
  Arch: pacman -S jdk17-openjdk
Then re-run, or set JAVA_HOME."

# Taken from beside javac, so the keystore is made by the same JDK that compiles
# — a JRE's keytool writing a store a newer JDK then rejects is a confusing way
# to fail at the last step.
keytool="$(dirname "$javac")/keytool"
[ -x "$keytool" ] || keytool="$(command -v keytool || true)"
[ -n "$keytool" ] || fail "no keytool beside $javac and none on PATH"

printf 'javac:       %s\n' "$javac"

printf 'build-tools: %s\nplatform:    %s\n' "$build_tools" "$android_jar"

# --- compile ----------------------------------------------------------------
rm -rf "$out"
mkdir -p "$out/res" "$out/classes" "$out/dex"

"$aapt2" compile --dir "$project/res" -o "$out/res/resources.zip"

"$aapt2" link \
    -o "$out/unsigned.apk" \
    -I "$android_jar" \
    --manifest "$project/AndroidManifest.xml" \
    --java "$out/gen" \
    --min-sdk-version 29 \
    --target-sdk-version 33 \
    "$out/res/resources.zip"

# `--release 11` rather than the JDK's default: d8 rejects class files newer
# than it understands, and the message it gives ("Unsupported class file major
# version") names the version rather than the cause.
mkdir -p "$out/gen"
find "$project/src" "$out/gen" -name '*.java' > "$out/sources.txt"
"$javac" \
    --release 11 \
    -Xlint:-options \
    -classpath "$android_jar" \
    -d "$out/classes" \
    @"$out/sources.txt"

"$d8" --release --min-api 29 --lib "$android_jar" --output "$out/dex" \
    $(find "$out/classes" -name '*.class')

# --- package ----------------------------------------------------------------
cp "$out/unsigned.apk" "$out/with-dex.apk"
(cd "$out/dex" && zip -q -X "$out/with-dex.apk" classes.dex)

"$zipalign" -f 4 "$out/with-dex.apk" "$out/aligned.apk"

# A local debug key. Never checked in, never shared: it exists because Android
# refuses to install an unsigned APK, not because anything here is trusted by
# virtue of the signature.
keystore="$out/../artist-android-debug.keystore"
if [ ! -f "$keystore" ]; then
    "$keytool" -genkeypair \
        -keystore "$keystore" -storepass artistdebug -keypass artistdebug \
        -alias artist -keyalg RSA -keysize 2048 -validity 10000 \
        -dname "CN=artist debug, OU=computer-use, O=artist, C=GB"
fi

"$apksigner" sign \
    --ks "$keystore" --ks-pass pass:artistdebug --key-pass pass:artistdebug \
    --out "$out/artist-bridge.apk" \
    "$out/aligned.apk"

printf '\nbuilt: %s\n' "$out/artist-bridge.apk"
printf 'install it with: artist computer android install-bridge\n'
