# Multilingual README Layout Design

## Goal

Reduce root-directory clutter without weakening the English and Simplified Chinese entry points or breaking repository links, release packaging, and documentation checks.

## Layout

Keep the canonical English README and complete Simplified Chinese mirror at the repository root:

```text
/
├── README.md
├── README.zh-CN.md
└── docs/
    └── readme/
        ├── README.ja.md
        ├── README.es.md
        ├── README.pt-BR.md
        ├── README.fr.md
        └── README.ru.md
```

The Japanese, Spanish, Brazilian Portuguese, French, and Russian mirrors move with Git history preserved.

## Link Contract

- Every README keeps the complete seven-language switcher.
- Root READMEs link translated mirrors through `docs/readme/`.
- Moved READMEs link English and Chinese through `../../` and link sibling translations directly.
- Repository-relative links inside moved files are rebased from the repository root to `../../`.
- Active product, packaging, test, and contributor-facing files must not retain broken references to the five former root paths. Historical specifications and plans are excluded.

## Compatibility

`README.md` and `README.zh-CN.md` remain unchanged in location, so Cargo metadata, npm packaging, release archives, pull-request guidance, and existing tests that treat English and Chinese as first-class documents keep their current contract. Only consumers that reference one of the five moved files require updates.

GitHub does not provide redirects for moved repository Markdown files. The language switcher and all tracked internal references therefore move atomically in one commit.

## Verification

Run the smallest checks covering the affected surface:

1. Search active tracked files for stale root references to the five moved READMEs, excluding historical specifications and plans.
2. Validate every local Markdown link and HTML `href` or `src` reference in all seven README files resolves from its new location; verify every entry in each language switcher individually.
3. Run the existing npm package and documentation-policy checks only when their inputs or assertions are affected.
4. Confirm `git diff --check` and inspect the rename-aware diff.

No README content rewrite, new localization system, redirect layer, or documentation generator is included.
