# Old-config compatibility fixtures

Each `vX.Y.Z/` directory will hold the configuration file(s) a release
persisted, hand-derived from that tag's source, for the daemon's config tests
(`just config-compat`) to prove the current code still loads configs written by
older releases — it must load, migrate, or reset, never crash.

**On every release, add a `vX.Y.Z/` directory** with the `daemon.ron` that
version wrote. Do not reformat existing files — they represent real on-disk
user configs.

## No daemon corpus yet

The `v0.1.x` directories are inherited from Super STT, whose `daemon.toml`
described recording, typing, and transcription. Super TTS renamed that section
to `[synthesis]` and dropped those keys, so those files no longer describe any
config this daemon can be asked to load — keeping them would have tested a
migration path that no user can be on. They were removed rather than rewritten,
because a hand-edited fixture claiming to be "what v0.1.3 wrote" is worse than
no fixture at all.

Add `daemon.ron` starting with Super TTS's first release.

The applet's configs moved with the applet to
[super-cosmic-applet](https://github.com/super-libre/super-cosmic-applet),
which imports the ones the Super TTS applet wrote and tests them there.
