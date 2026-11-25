# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/lsdocs/lsdocs-plugin-sdk/compare/lsdocs-plugin-node/v0.1.0...lsdocs-plugin-node/v0.1.1) - 2026-11-25

### Other

- name the parser-myst builtin in place of parser-markdown ([#28](https://github.com/lsdocs/lsdocs-plugin-sdk/pull/28))
- *(deps)* update dependency @types/node to v26.6.7 ([#26](https://github.com/lsdocs/lsdocs-plugin-sdk/pull/26))
- *(deps)* update dependency @napi-rs/cli to v3.11.1 ([#22](https://github.com/lsdocs/lsdocs-plugin-sdk/pull/22))

## [0.1.0](https://github.com/lsdocs/lsdocs-plugin-sdk/releases/tag/lsdocs-plugin-node/v0.1.0) - 2026-11-22

### Added

- release automation ([#3](https://github.com/lsdocs/lsdocs-plugin-sdk/pull/3))
- tests/site for the toc ports, binding tasks, README, AGENTS.md
- *(render-html)* search
- *(transform-toc)* the toc as page data
- *(render-html)* the lsd- markup contract
- *(@lsdocs/plugin)* settings from .config/
- *(host)* plugin settings in .config/ (Phase 13)
- protocol 1.0-rc.0, finalized spec and docs site (Phase 9)
- *(parser-rst)* reStructuredText parser plugin (Phase 8e)
- *(bindings)* render/site for Python and Node renderers (Phase 7d)
- content-hash build cache (Phase 7b)
- adopt the shared document shape in plugins (Phase 6d-3 to 6d-5)
- *(doctree)* add the shared document shape (Phase 6d-1)
- add site navigation and a sitemap (Phase 6b)
- check links, fill cross-references and add backlinks (Phase 6a)
- *(bindings)* add Node.js bindings (Phase 5b)

### Fixed

- settings of the wrong kind are errors; stale options wording

### Other

- *(deps)* update dependency typescript to v7 ([#8](https://github.com/lsdocs/lsdocs-plugin-sdk/pull/8))
- rename sdk-dynamic, built-ins behind features
- the renamed pipeline methods
- *(@lsdocs/plugin)* follow the renamed pipeline methods
- *(protocol)* rename pipeline methods (Phase 13)
- *(bindings)* group bindings by library (Phase 10a)
- deny unwrap, expect and panic outside tests
- License lsdocs under Apache-2.0 OR MIT
- Scaffold the lsdocs workspace (roadmap Phase 0)
