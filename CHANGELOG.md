# Changelog

Notable changes to `rainmill-vault`. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/) as Cargo reads it: before 1.0 a minor
bump is breaking and a patch is additive.

## [Unreleased]

### Added

- First cut. A credential vault for Laterite: kinds and slots other modules declare, labelled credentials sealed at rest, pools a slot cycles through, an allowance a period, and a count of what each credential spent.
