#!/bin/bash
# A private headless Linux session for running and testing Marquee Player without a desktop,
# and in WSL without WSLg (nothing shows on the Windows desktop, nothing plays through WSLg's
# PulseAudio, nothing uses the Windows GPU): its own runtime folder, D-Bus session bus,
# PipeWire + WirePlumber with two null sinks ("Test Speakers", "Test Headphones"; the graph
# allows 44.1, 48, 88.2 and 96 kHz), headless Weston (Wayland, 1920 x 1080) and Xvfb (X11, :77),
# Mesa's software renderers only (llvmpipe, lavapipe).
#
#   packaging/linux/headless-session.sh start     # start it (as the user who runs the app)
#   packaging/linux/headless-session.sh run wayland|x11 <command...>
#   packaging/linux/headless-session.sh stop
#
# `run` gives the command the session's environment, with WAYLAND_DISPLAY or DISPLAY set, in a
# private mount namespace that hides WSL's markers (/proc/sys/fs/binfmt_misc/WSLInterop,
# /run/WSL): Slint forces X11 under WSL, and with them hidden the app also cannot start Windows
# programs through WSL interop. That needs root (or `unshare` rights); elsewhere it runs as is.
# Needs: dbus, pipewire, wireplumber, weston, xvfb, mesa (llvmpipe, lavapipe), util-linux.
# For audio tests, `pw-record --target mp-null-a -P stream.capture.sink=true out.wav` records
# what plays on "Test Speakers".
set -e
R=${MP_SESSION_DIR:-$HOME/.cache/mp-headless-session}
envs() {
  cat <<E
unset PULSE_SERVER WAYLAND_DISPLAY DISPLAY DBUS_SESSION_BUS_ADDRESS WSL2_GUI_APPS_ENABLED
export XDG_RUNTIME_DIR=$R
export XDG_CONFIG_HOME=$R/config
export DBUS_SESSION_BUS_ADDRESS=unix:path=$R/bus
export GALLIUM_DRIVER=llvmpipe LIBGL_ALWAYS_SOFTWARE=1 MESA_LOADER_DRIVER_OVERRIDE=llvmpipe
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json
E
}
bg() { local name=$1; shift; nohup setsid "$@" > "$R/$name.log" 2>&1 < /dev/null & echo $! > "$R/$name.pid"; }
case "$1" in
start)
  eval "$(envs)"
  mkdir -p "$R/config/pipewire/pipewire.conf.d" && chmod 700 "$R"
  cat > "$R/config/pipewire/pipewire.conf.d/10-mp-test.conf" <<C
context.properties = {
    default.clock.rate = 48000
    default.clock.allowed-rates = [ 44100 48000 88200 96000 ]
}
context.objects = [
    { factory = adapter args = { factory.name = support.null-audio-sink node.name = "mp-null-a" node.description = "Test Speakers"
      media.class = "Audio/Sink" audio.position = [ FL FR ] monitor.channel-volumes = true node.driver = true } }
    { factory = adapter args = { factory.name = support.null-audio-sink node.name = "mp-null-b" node.description = "Test Headphones"
      media.class = "Audio/Sink" audio.position = [ FL FR ] monitor.channel-volumes = true node.driver = true } }
]
C
  setsid dbus-daemon --session --address="unix:path=$R/bus" --fork --print-pid > "$R/dbus.pid"
  bg pipewire pipewire
  sleep 1
  bg wireplumber wireplumber
  bg weston weston --backend=headless --renderer=gl --width=1920 --height=1080 --socket=wayland-mp --idle-time=0
  bg xvfb Xvfb :77 -screen 0 1920x1080x24 -nolisten tcp
  sleep 2
  echo "session in $R"
  ;;
run)
  eval "$(envs)"
  disp=$2; shift 2
  if [ "$disp" = x11 ]; then export DISPLAY=:77; else export WAYLAND_DISPLAY=wayland-mp; fi
  if [ -e /proc/sys/fs/binfmt_misc/WSLInterop ] || [ -e /run/WSL ]; then
    exec unshare -m --propagation private bash -c 'mount -t tmpfs none /run && mount -t tmpfs none /proc/sys/fs/binfmt_misc && exec "$@"' run "$@"
  fi
  exec "$@"
  ;;
stop)
  for p in xvfb weston wireplumber pipewire dbus; do
    [ -f "$R/$p.pid" ] && kill "$(cat "$R/$p.pid")" 2>/dev/null && echo "stopped $p"
    rm -f "$R/$p.pid"
  done
  ;;
*)
  echo "usage: $0 start | run wayland|x11 <command...> | stop" >&2
  exit 2
  ;;
esac
