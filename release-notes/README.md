# Release Notes Process

This directory contains release notes for the Restate Rust SDK, organized to track changes between releases.

## Structure

```
release-notes/
├── README.md          # This file
├── v0.8.0.md          # Consolidated release notes for v0.8.0
├── v0.9.0.md          # Consolidated release notes for v0.9.0
└── unreleased/        # Release notes for changes not yet released
    └── *.md           # Individual release note files
```

## Adding Release Notes

When making a significant change that affects SDK users, create a release note file in the `unreleased/` directory:

1. **Create a new file** in `unreleased/` with a descriptive name:
   - Format: `<issue-number>-<short-description>.md`
   - Example: `83-add-invocation-id-to-context.md`

2. **Structure your release note** with the following sections:
   ```markdown
   # Release Notes for Issue #<number>: <Title>

   ## Behavioral Change / New Feature / Bug Fix / Breaking Change

   ### What Changed
   Brief description of what changed

   ### Why This Matters
   Explain the impact and reasoning

   ### Impact on Users
   - How this affects existing code
   - How this affects new projects
   - Any migration considerations

   ### Migration Guidance
   Steps users should take if needed, including:
   - Dependency changes (Cargo.toml)
   - Code changes
   - MSRV or toolchain changes

   ### Related Issues
   - Issue #XXX: Description
   ```

3. **Commit the release note** with your changes:
   ```bash
   git add release-notes/unreleased/<your-file>.md
   git commit -m "Add release notes for <change>"
   ```

## Release Process

When creating a new release:

1. **Review all unreleased notes**: Check `unreleased/` for all pending release notes

2. **Create a consolidated release notes file** (`v<version>.md`) with the following structure:

   ```markdown
   # Restate Rust SDK v<version> Release Notes

   ## Highlights
   - 3-5 bullet points summarizing the most important changes

   ## Table of Contents
   (Links to all sections below)

   ## Breaking Changes
   (Items sorted by impact, most impactful first)

   ## Deprecations

   ## New Features
   (Items sorted by impact, most impactful first)

   ## Improvements
   ### API Changes
   ### Performance
   ### Testing / Testcontainers

   ## Bug Fixes
   ```

3. **Consolidation guidelines**:
   - Sort items by impact within each category (most impactful first)
   - Preserve all migration guidance, configuration examples, and code snippets
   - Keep all related issue/PR links
   - For items spanning multiple categories (e.g., both breaking change and new feature), place in the primary category with full context

4. **Delete the individual unreleased files** after consolidation:
   ```bash
   rm release-notes/unreleased/*.md
   ```

5. **Use the consolidated notes** to prepare:
   - GitHub release description
   - Documentation updates
   - Blog post content (if applicable)

## Guidelines

### When to Write a Release Note

Write a release note for:
- **Breaking changes**: Any change that requires user action or breaks existing code (e.g., MSRV bump, API removal)
- **Behavioral changes**: Changes to defaults or runtime behavior
- **New features**: User-facing API additions or capabilities
- **Important bug fixes**: Fixes that significantly impact reliability or correctness
- **Deprecations**: APIs or features being deprecated

### When NOT to Write a Release Note

Skip release notes for:
- Internal refactoring with no user impact
- Test changes
- Documentation-only changes (unless significant)
- Minor dependency updates
- CI/build system changes

### Writing Style

- **Be clear and concise**: Users should quickly understand the change
- **Focus on impact**: Explain what users need to know and do
- **Provide examples**: Include Rust code snippets or `Cargo.toml` changes
- **Link to documentation**: Reference docs.rs or detailed docs when available
- **Be honest about breaking changes**: Don't hide backwards-incompatible changes

## Examples

### Breaking Change Example
```markdown
# Release Notes for Issue #82: Bump Rust edition to 2024

## Breaking Change

### What Changed
The SDK now requires Rust edition 2024 and a minimum supported Rust version (MSRV) of 1.85.0.

### Impact on Users
- Projects using an older Rust toolchain will fail to compile
- The `edition` field in your `Cargo.toml` does not need to change, but your toolchain must be >= 1.85.0

### Migration Guidance
Update your Rust toolchain:

\```bash
rustup update stable
\```

Verify your version:

\```bash
rustc --version  # Must be >= 1.85.0
\```
```

### New Feature Example
```markdown
# Release Notes for Issue #83: Add invocation ID to context

## New Feature

### What Changed
All handler context types now expose an `invocation_id()` method that returns the current
Restate invocation ID.

### Why This Matters
The invocation ID is useful for logging, debugging, and correlating requests across services.

### Impact on Users
- No breaking changes; this is a purely additive API
- Available on all context types: `Context`, `ObjectContext`, `SharedObjectContext`,
  `WorkflowContext`, `SharedWorkflowContext`

### Related Issues
- PR #83
```
