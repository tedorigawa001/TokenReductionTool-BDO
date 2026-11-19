# Ruby on Rails

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `rake_cmd.rs` filters Minitest output via `rake test` / `rails test`; state machine text parser, failures only (85-90% reduction)
- `rspec_cmd.rs` uses JSON injection (`--format json`) with text fallback; failures only (60%+ reduction)
- `rubocop_cmd.rs` uses JSON injection, groups by cop/severity (60%+ reduction)
- All three modules use `ruby_exec()` from `utils.rs`; a Gemfile never implicitly adds `bundle exec`
- Hooks preserve an explicitly requested `bundle exec` and the original Rake/Rails or local-binstub choice. To opt into a bundled context manually, use e.g. `bundle exec bdo rspec ...`
- TOML filter `bundle-install.toml` strips `Using` lines from `bundle install`/`update` (90%+ reduction)
