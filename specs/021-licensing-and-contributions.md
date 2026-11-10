# 021 — Keeping the right to relicense

> Written for: whoever signs the contribution terms, and whoever has to
> explain the decision later.

The requirement is specific: **we must be able to change the licence in
the future, and everything released up to that moment stays available
under the licence it was released with.** The second half is automatic;
the first half is not, and it is lost by accident rather than by decision.

## Where we stand today

| | |
|---|---|
| Copyright holders | **One.** `git log --all --format='%an <%ae>'` returns a single person under two identities |
| Third-party source in the tree | **None** vendored. Dependencies are external and permissive |
| Copyright headers / SPDX | **Every file that holds logic**, checked by `scripts/spdx.sh` |
| Outbound licence | AGPL-3.0-or-later, except `hidpp` (MIT) |

So the right to relicense exists **right now, unconditionally**: a sole
copyright holder may offer the same code under any terms, as often as they
like. Publishing under AGPL does not spend that right.

**What spends it is one external contribution we do not own.** The window
does not close "eventually" — it closes the first time someone else's code
is merged, which is days after the repository goes public. That is why
this is decided before step 10 of [[018-open-source]], not after.

## The half that is automatic: released stays released

An AGPL grant already made is perpetual and irrevocable for the version it
was made for. A relicence is **forward-only**: it cannot reach the
binaries in the published Releases, the tags, or any clone. Whoever has
`v0.2.8` keeps AGPL rights to `v0.2.8` forever, and that is true whether
we like it or not.

We state it on purpose rather than leaving it implied, because it is what
makes such a move defensible if it ever happens, and because stating it
costs nothing we could have kept.

## The instrument

**Decision: a contributor licence agreement with a sublicensable grant,
in place before the repository becomes public.**

What it has to say, in substance: the contributor keeps their copyright,
and grants us a perpetual, worldwide, non-exclusive, royalty-free,
**sublicensable** licence to use, reproduce, modify and distribute the
contribution **under any licence terms**. That last clause is the whole
point — without it the contribution is AGPL-only and so is anything
containing it.

The alternatives, and why not:

| Instrument | Preserves relicensing? | Friction | Verdict |
|---|---|---|---|
| **CLA with sublicensable grant** | Yes | A signature, once, automated by a bot | **Chosen** |
| DCO sign-off (`Signed-off-by`) | **No.** It certifies provenance and the right to submit under the *project's* licence, nothing more | A commit trailer | Keep **as well**, for provenance; it is not a substitute |
| Copyright assignment | Yes, maximally | High — individuals and employers both balk | Rejected: buys little over a CLA and costs contributors |
| Nothing, and simply refuse external PRs | Yes, trivially | Zero, until the first person offers a fix and we say no | Rejected: it makes the "open source for contributors" argument dishonest |

`AGPL-3.0-**or-later**` is already in place and is worth keeping, but it
only permits moving to a later AGPL — not to a different family. It is not
the mechanism here.

## The constraint no CLA can fix

**FFmpeg.** The macOS packages link Homebrew's `ffmpeg@7`, which is
GPL-3.0-or-later ([[../THIRD-PARTY]]). That constrains the licence of
**what we distribute**, no matter who owns our own code: a binary linked
against a GPL-3 library cannot be shipped under BSL, SSPL, or any
proprietary licence. Owning 100% of our source does not help, because the
constraint comes from the other side of the link.

Which reframes [[020-native-mac]]. Dropping FFmpeg for ScreenCaptureKit
and VideoToolbox was written up as dependency hygiene — fewer failure
modes, no Homebrew in the install path, no `install_name_tool` rewriting.
It is also **a prerequisite for licensing freedom**, and that moves it
from cleanup to strategy. Linux and Windows link no FFmpeg, so today the
Mac alone decides what the whole distribution can be.

If 020 does not happen, the fallback is to split the distribution: the Mac
package stays GPL-compatible while the rest moves. That is legally
possible and operationally unpleasant — two licences, two sets of notices,
and a user-visible difference between platforms for a reason no user
cares about. Prefer removing the constraint to managing it.

## What has to exist in the repository

1. **`CONTRIBUTING.md` stating the inbound terms**, so the grant is where
   a contributor actually looks, not only in a signing flow.
2. **A CLA bot on pull requests**, because an unautomated agreement is an
   agreement nobody signed.
3. **`SPDX-License-Identifier` headers** in every source file, plus a
   `NOTICE` naming the copyright holder. Two reasons, and both matter
   later: AGPL compliance expects the notice, and a relicence is an
   audit — "which files, under which terms, contributed by whom" has to be
   answerable mechanically rather than by reading `git log`. **Done:**
   `NOTICE`, and 128 headers written by
   [`scripts/spdx.sh`](../scripts/spdx.sh), which also checks them from
   `check.sh` on every run.
4. **`hidpp` stays MIT and separate.** It is the piece we want other
   projects to take, permissive means no constraint in either direction,
   and keeping it out of the relicensable core is deliberate.

## Adversarial review

### 1. "A CLA reduces contributions, and we said contributors were the point"

True, and measurably so: individuals skip projects that ask for a
signature, and corporate contributors route it through legal, which can
take months.

*What survives:* [[019-one-onboarding]] already argues that the realistic
contributor pool here is near zero — three machines, a Unifying receiver,
an NVIDIA GPU, GNOME on Wayland. So the CLA costs almost nothing **today**,
and that is exactly why to adopt it today. The honest framing is
uncomfortable and worth writing down: it is cheap because nobody is
contributing yet, and the cost grows with success. If contribution volume
ever becomes a real metric, revisit it — dropping a CLA later is easy,
adding one later is not.

### 2. "The right to relicense invites using it, and that burns the
community"

The precedents are consistent and recent: MongoDB AGPL→SSPL, Elastic
Apache→ELv2 which produced OpenSearch, HashiCorp MPL→BSL which produced
OpenTofu, Redis BSD→RSAL/SSPL which produced Valkey. Every one of them
kept the legal right and paid a reputational price for exercising it.

*What survives:* this organisation ships a `valkey-operator` — we build on
the fork that a relicence created, so the dynamic is not theoretical here.
The conclusion is not "do not keep the right"; it is that **the right and
the wisdom are separate decisions.** Keeping the option is cheap.
Exercising it costs trust, invites a fork of exactly the part that has
adoption, and should require a reason stronger than "we can".

### 3. "This is governance theatre for a project with one author"

Also fair. One author, no contributions, and a document about contribution
terms.

*What survives:* the cost is one file and a bot, and the thing it protects
cannot be recovered after the fact. Collecting agreements retroactively
from people who have already contributed is the task that does not finish
— and the asymmetry between "cheap now" and "impossible later" is the
whole argument.

### 4. "AGPL already protects the business; relicensing is a solution
looking for a problem"

AGPL's section 13 obliges whoever offers the software over a network to
publish their modifications, which is aimed at exactly our relay
([[017-product]]). For a competitor hosting `oxdesk-encontro`, that is
real protection.

*What survives:* AGPL protects against hosting; it does not enable selling
non-AGPL terms to a customer whose legal department forbids AGPL in the
building. That customer exists, and the Enterprise relay is where the
money was always going to be. Dual licensing is the normal answer, and
dual licensing requires owning the rights.

## Order of work

1. ~~**`CONTRIBUTING.md` with the inbound grant.**~~ **Done:** the grant
   is stated in substance, in the second paragraph, naming oxlayer as the
   grantee. That is also what settles whose name the `NOTICE` carries —
   the file a contributor reads already answered it.
2. ~~**SPDX headers and `NOTICE`.**~~ **Done.** `scripts/spdx.sh` owns
   both halves: it writes the headers (`--fix`) and it is the check, wired
   into `check.sh` so a file added next week fails loudly instead of
   shipping undeclared.

   The set it enforces is files that hold logic — `.rs`, `.sh`, `.ps1`,
   `.js`, `.swift`, `.html`, and `linux/oxdesk-panel`. Deliberately out:
   `Cargo.toml`, which already declares `license =` and would become a
   second source of truth to keep in sync; `.desktop`, `.service`,
   `.rules` and `.plist`, parsed by systemd, udev and launchd, which are
   strict about what they accept and hold no logic to attribute; and
   Markdown, covered by `LICENSE` and `NOTICE` without putting
   boilerplate at the top of 55 pages.

   `hidpp/` gets `MIT` and everything else `AGPL-3.0-or-later`, and the
   check compares the identifier rather than merely finding one — an AGPL
   header inside `hidpp/` fails instead of quietly misdeclaring the
   licence. Both failure directions were tested by breaking them on
   purpose, because a check that has never failed is a check nobody has
   tested.
3. **CLA bot on pull requests.** *Check:* a throwaway PR from another
   account is blocked until signed.
4. **Then [[020-native-mac]]**, which is what converts the right into a
   real option rather than a theoretical one.

## See also

- [[018-open-source]] — the plan this closes an open question in
- [[020-native-mac]] — the dependency that constrains the outbound licence
- [[../THIRD-PARTY]] — what is inside what we distribute, and under which
  terms
