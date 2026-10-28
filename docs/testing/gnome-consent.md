# Live GNOME consent regression

The `GNOME Shell live consent` CI job runs GNOME 46 on Ubuntu 24.04 with an Xvfb X11 display and Mesa software rendering. It loads the unchanged extension into the actual Shell process. Screenshot OCR locates the rendered Allow/Deny buttons; XTest mouse and keyboard input choose them. No Shell Eval API, dialog stub, or extension monkey-patch is used.

The test exercises all five protected operations, denial caching, Escape/default Return, the 20-second timeout, peer isolation, pending-client disconnect, and grant revocation when the extension is disabled and enabled. An xterm supplies a real compositor window for focus, position, and size assertions. Logs, screenshots, OCR output, the Shell version, and source/copied extension hashes are retained as CI artifacts.

## Run in a disposable VM or container

A normal Ubuntu 24.04 container can supply these packages, provided its host permits Unix-domain sockets and local X11 connections. It does not need a physical GPU. Run only in a disposable environment: Shell is a desktop process, and this harness drives its display. It creates private XDG configuration/data/cache directories, an ephemeral display, and a private session bus.

Install the packages as the container administrator:

```sh
apt-get update
apt-get install --no-install-recommends -y gnome-shell dbus-x11 xvfb xauth xterm xdotool scrot tesseract-ocr python3-gi python3-pil gir1.2-gtk-3.0 libgl1-mesa-dri
```

Then, as an ordinary test user in the checkout:

```sh
bash scripts/gnome_shell_live_test.sh "$PWD"
```

Inspect `gnome-live-artifacts/assertions.log` and screenshots. A script's presence or syntax pass is not live validation: the entire run must pass. The existing `GNOME authorization` GJS fixture exercises real D-Bus dispatch but stubs Shell UI and is a separate gate.

## Coverage limits

This qualifies GNOME 46 modal and compositor behavior on X11. It does not qualify every supported GNOME version, Wayland, a physical monitor/GPU, or installation into an existing user's desktop. Test those surfaces separately when they are part of a release's acceptance scope. An environment that blocks AF_UNIX socket creation cannot run this test, even inside a nested container.
