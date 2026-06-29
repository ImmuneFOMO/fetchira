# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/ImmuneFOMO/fetchira/releases/tag/v0.1.0) - 2026-06-29

### Added

- auto-release pipeline, prebuilt install, and self-update

### Fixed

- *(grok_web)* add x-xai-request-id, honest 403 message
- *(router)* reserve quota before the call to close concurrent overshoot
- ui offline notification and input for api key
- *(router)* skip accounts with unresolved keys instead of aborting startup

### Other

- first fully working version
