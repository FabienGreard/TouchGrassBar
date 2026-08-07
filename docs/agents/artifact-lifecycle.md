# Research and Spike Lifecycle

Research notes and spike artifacts are temporary evidence owned by a GitHub
issue. The main branch keeps current product knowledge, not an investigation
archive.

## Create

Before an artifact enters a pull request, give it these fields:

- **Status:** active research or active spike
- **Related issues:** one or more open GitHub issues that own the work
- **Cleanup condition:** one checkable condition for deletion
- **Promotion target:** the ADR, runbook, production module, test, or issue that
  will receive each durable result

Keep throwaway implementation code on a throwaway branch. Link that branch from
the related issue. Main can receive validated production code and tests.

## Close

Before all related issues close or the implementation pull request merges:

1. Move durable decisions into an ADR, runbook, `CONTEXT.md`, production code,
   or tests.
2. Delete the research note or spike artifact.
3. If unresolved work still needs the artifact, link it to an open follow-up
   issue and replace the cleanup condition with that issue's exit condition.

An artifact with no open owner issue is orphaned and cannot remain on main.

## Review

A pull-request review is complete only when every changed research or spike
artifact has one explicit disposition: delete now, promote now, or retain under
an open issue with a cleanup condition.
