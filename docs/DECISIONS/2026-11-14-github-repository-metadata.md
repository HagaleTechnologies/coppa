# GitHub repository metadata: description, topics, homepage, Discussions and social preview

Recorded by COP-74 on 2026-11-14. The source is row H-111 of the
[product review](../reviews/2026-09-05-product-ux-market-review.md) (section 3.6): a
description, topics, a homepage, a social preview, Discussions, issue templates including a
field report, `CHANGELOG.md` and a `docs/README.md` index.

The repository files land in the COP-74 pull request: the issue forms under
`.github/ISSUE_TEMPLATE/`, the [docs index](../README.md) and the README's Community section.
The settings below are repository-admin settings that no file change reaches. **The live
repository keeps its old values until an admin runs [Apply](#apply).** When COP-74 was
recorded those values were the description "Ham Radio Software Modem Reference", no topics,
no homepage, Discussions off, no custom social preview, no tags and no releases.

## Scope

A Catalyst Cloud dependency-grooming comment on COP-74, 2026-10-08T04:20:48Z:

> "'a real release' comes from COP-47, and the 'Winlink-ready' wording waits on H-035
> (COP-46). The metadata cleanup itself can start, so no relation added."

The first release therefore belongs to COP-47 and its runbook,
[RELEASING.md](../RELEASING.md), not to this record. COP-47 merged as PR #244 and left the
tag push as an owner step, so the repository still shows "No releases published".

## Decisions

- **D1 Description.** `Open-source OFDM HF data modem for amateur radio, in Rust -- VARA-style TCP API, C FFI`.
  This is COP-74's string, 86 characters. H-111 writes an em dash where the ticket writes `--`;
  this record uses the ticket's two hyphens. It keeps the word "style": the
  [README](../../README.md) and [CONTRIBUTING.md](../../CONTRIBUTING.md) say the modem is not
  RF-compatible with VARA and that only the TCP interface is VARA-style.
- **D2 No "Winlink-ready".** The ticket allows the phrase "only after H-035 passes". H-035
  (COP-46) merged as PR #161, so the gate is met, but the ticket permits the phrase and does
  not require it. The only Winlink evidence is Pat against two `coppad` processes joined by a
  userspace audio loop on one machine, and "a Winlink RMS gateway running VARA, cannot hear a
  coppa station" ([PAT.md](../PAT.md)). A visitor would read "Winlink-ready" as "works with
  the Winlink network", which is not true. CONTRIBUTING.md forbids aspirational framing.
- **D3 Topics.** The eleven from H-111: `ham-radio`, `amateur-radio`, `hf`, `ofdm`, `ldpc`,
  `modem`, `winlink`, `rust`, `dsp`, `tnc`, `ax25`. `winlink` stays because coppa carries
  Winlink P2P sessions through Pat.
- **D4 Homepage.** `https://github.com/HagaleTechnologies/coppa/blob/main/docs/README.md`, the
  docs index. The repository has no Pages site, and the documentation site (H-118) is on hold
  ([ROADMAP.md](../../ROADMAP.md), "On hold").
- **D5 Social preview.** A 1280 by 640 PNG with a dark background, the word "coppa", the line
  "Open-source OFDM HF data modem for amateur radio", and a small line "Rust · VARA-style TCP
  API · C FFI · MIT OR Apache-2.0". It carries no throughput or SNR numbers, no
  "Winlink-ready", and never "VARA" without "-style". GitHub's upload page states the accepted
  formats and size limit. An admin makes the image and uploads it under Settings, General,
  Social preview. The repository holds no copy of it.
- **D6 Discussions on.** The README's Community section and the issue chooser link to
  `/discussions`. Those links open GitHub's 404 page until Discussions is enabled, so an admin
  enables it before the COP-74 pull request merges.
- **D7 Issue forms.** `bug.yml`, `feature.yml`, `field-report.yml` and `config.yml` under
  `.github/ISSUE_TEMPLATE/`. A repository's own templates replace the organisation's defaults.
  Blank issues stay enabled: the
  [clean-room policy](2026-11-09-clean-room-and-licensing-policy.md) asks contributors to
  request copy approval "in a GitHub issue", and [SECURITY.md](../../SECURITY.md) falls back
  to a minimal public issue when private reporting is off. The forms use only labels that
  exist: `bug` and `enhancement`. The field report carries no label, because no
  `field-report` label exists; its `[field report]` title prefix makes it searchable.
- **D8 CHANGELOG.md.** It already exists. COP-74 links it from the README and the docs index
  and leaves its content alone. Its "Unreleased" headings belong to the release step in
  RELEASING.md.

## Apply

The pipeline that produced COP-74 holds no token that can write repository settings, so none
of these steps has been run. An admin does them in this order.

1. **Before the COP-74 pull request merges,** turn on Discussions under Settings, General,
   Features, Discussions (D6). Until then the README's Community link and the issue chooser's
   "Questions and ideas" link open GitHub's 404 page.
2. **After it merges,** run the command below once. It waits for the merge because the
   homepage URL points at `docs/README.md` on `main`, which does not exist until then. Its
   `--enable-discussions` flag changes nothing if step 1 is done, and turns Discussions on if
   it was missed.
3. Upload the D5 image under Settings, General, Social preview.

```bash
gh repo edit HagaleTechnologies/coppa \
  --description "Open-source OFDM HF data modem for amateur radio, in Rust -- VARA-style TCP API, C FFI" \
  --homepage "https://github.com/HagaleTechnologies/coppa/blob/main/docs/README.md" \
  --enable-discussions \
  --add-topic ham-radio,amateur-radio,hf,ofdm,ldpc,modem,winlink,rust,dsp,tnc,ax25
```

## Check

```bash
gh api repos/HagaleTechnologies/coppa --jq '{description, homepage, topics, has_discussions}'
gh api graphql -f query='{repository(owner:"HagaleTechnologies",name:"coppa"){usesCustomOpenGraphImage}}'
```

The settings are in place when the first command shows the D1 description, the D4 homepage,
the eleven D3 topics and `"has_discussions": true`, and the second shows
`"usesCustomOpenGraphImage": true`.

## Rejected alternatives

- **The `sdr` topic.** No document in the repository mentions SDR, so the topic would bring
  visitors looking for something coppa does not describe.
- **The `vara` topic.** It implies compatibility that the clean-room policy and the README
  rule out.
- **"Winlink-ready" in the description.** See D2.
- **An image file in the repository.** GitHub takes the social preview as an upload in
  Settings, not from a file in the tree, and a copy in the tree would drift from what is
  uploaded.
- **Rewriting CHANGELOG.md in the Keep a Changelog format.** [SPEC.md](../SPEC.md) requires
  `CHANGELOG.md` to carry set fields for each air-protocol break, so a format rewrite is its
  own change, not part of this metadata cleanup.

## Upkeep

- Revisit D2 once COP-20 records an over-the-air result.
- Revisit D4 when H-118 ships a documentation site.
- Keep the description and the D5 image in step: change both, or neither.
