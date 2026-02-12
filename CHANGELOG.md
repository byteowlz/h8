# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.3] - 2026-02-12

### Fixed

- **Date year default**: Month-day dates without a year (e.g., `june 9`, `9.6`) now correctly default to the current year instead of the previous year

## [0.5.2] - 2026-02-12

### Added

- **Meeting cancellation with attendee notifications**: `h8 cal cancel <id>` properly cancels meetings and sends cancellation emails to all attendees (unlike delete which silently removes)
- **Bulk meeting cancellation**: `h8 cal cancel -q today` cancels all meetings matching a query, with `--dry-run` to preview
- **Email address cache**: `h8 addr` command to search frequently used email addresses from your sent/received mail
- **Bulk mail moves by search**: `h8 mail move -q "from:newsletter" --to newsletters` moves all matching messages
- **German date formats**: support for `28.01`, `28.01.2026`, and `mittwoch` in date parsing
- **Day offset syntax**: `+2`, `-1` for relative dates (e.g., `h8 agenda +2` for day after tomorrow)
- **Natural language agenda dates**: `h8 agenda tomorrow` or `h8 agenda friday` now works

### Changed

- Weekday parsing now returns the most recent occurrence (today or earlier) instead of always looking forward
- Date parsing unified across all commands for consistent behavior
- Skill documentation condensed to essential commands only
