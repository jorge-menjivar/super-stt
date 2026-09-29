# Old-config compatibility fixtures

Each `vX.Y.Z/` directory holds the configuration file(s) that release persisted,
hand-derived from that tag's source. They are loaded by the daemon's config
tests (`just config-compat`) to prove the current code still loads configs
written by older releases — it must load, migrate, or reset, never crash.

**On every release, add a `vX.Y.Z/` directory** with the `daemon.toml` that
version wrote. Do not reformat existing files — they represent real on-disk
user configs.

The COSMIC applet's configs moved with the applet to
[super-cosmic-applet](https://github.com/super-libre/super-cosmic-applet),
which imports the ones the Super STT applet wrote and tests them there.
