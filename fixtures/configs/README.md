# Old-config compatibility fixtures

Each `vX.Y.Z/` directory holds the configuration file(s) that release persisted,
hand-derived from that tag's source. They are loaded by the daemon and applet
config tests (`just config-compat`) to prove the current code still loads configs
written by older releases — it must load, migrate, or reset, never crash.

**On every release, add a `vX.Y.Z/` directory** with the configs that version
wrote (`daemon.toml`, `applet-full.toml`). Do not reformat existing files — they
represent real on-disk user configs.
