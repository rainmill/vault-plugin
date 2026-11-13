# Changelog

Notable changes to `rainmill-vault`. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/) as Cargo reads it: before 1.0 a minor
bump is breaking and a patch is additive.

## [Unreleased]

### Added

- First cut. A credential vault for Laterite: kinds and slots other modules declare, labelled credentials sealed at rest, pools a slot cycles through, an allowance a period, and a count of what each credential spent.
- Usage page for each slot: charts by meter, by credential and of calls against errors over a chosen period, with a price a unit, an estimated cost and a projection for the month.
- `KindReg::category` files a kind under a `Category`; with several kinds the Credentials screen offers them from one New credential menu, grouped by category.
- `SlotReg::unit` and `SlotReg::meter`, and `Lease::used_as(meter, units)` to count a use under a meter.
