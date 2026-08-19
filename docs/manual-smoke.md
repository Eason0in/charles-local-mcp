# Explicit manual Charles/Android smoke test

This test mutates a real Android device and starts Charles. Never run it as
part of the automated suite. Use only a dedicated test device and a generic or
authorized profile.

1. Build `target/debug/charles-local-mcp` and prepare a valid profile with an
   optional HTTPS `verificationUrl`.
2. Run `status --json` and `devices list --json`. If an unrelated active
   session exists, stop and resolve it first.
3. Run `doctor --json` and confirm macOS plus Charles `4.6.8`.
4. Run `setup plan --profile PROFILE --platform android --device DEVICE
   --json`, inspect its actions, then apply its single-use token.
5. Confirm the managed Charles GUI is running, the owned ADB reverse exists,
   and Android proxy points at loopback. Confirm no user-owned Charles process
   or original config was changed.
6. If the HTTPS check returns a CA checkpoint, install/trust only the expected
   Charles CA manually and run `setup resume --token TOKEN --json`.
7. In all success and failure cases, run `cleanup plan --json`, inspect it, and
   apply its token. Confirm the prior proxy value is restored only when the
   configured value was unchanged, the owned reverse is removed, the managed
   Charles process stops, and user Charles remains untouched.

If cleanup fails, do not delete the state directory. Preserve it and retry so
ownership and compare-and-swap guards remain available.
