# Handover: koto#609 continuation 1, at the end of scrutiny

Branch `feature/609-declared-lookup`. The code head is `dfcc937`
(dfcc9375c46b46953d57ebc40b0a5b35e06092d5), and this file's commit sits on
top of it. The branch was 0 behind main at `802b86f`, and main was merged in
without a force push. No pull request exists yet.

## Done

- `1e4face` adds the declared branch of `held_elsewhere`
  (`src/session/cloud.rs`: `reads_declared`, `DeclarationAsk`,
  `get_declaration_retrying`), plus `get_declaration`, `unanswered_get` and
  `MAX_DECLARATION_CARRY` in `src/session/name_index.rs`. It also adds the
  shared helper `tests/support/declaration.rs`, the nine named tests, and a
  CHANGELOG entry. #608's test
  `a_planted_declaration_changes_no_reply_and_skips_no_request` is rewritten
  in place as `a_planted_declaration_changes_only_an_unindexed_session_s_reply`.
- `dfcc937` adds `k_leftover_entries_on_a_declared_bucket_cost_k_confirming_listings_in_two_waves`
  and `a_declared_bucket_with_no_memo_reads_in_one_round_trip_whatever_the_top_level_listing_does`,
  which answer scrutiny round 1.
- Targeted runs at `cae09e6`: cloud_bound_dir_test 35/35,
  cloud_index_lookup_test 24/24, cloud_name_index_test 13/13. At `dfcc937`,
  cloud_index_lookup_test is 26/26. fmt and clippy are clean.
  `tests/cloud_bound_dir_test.rs` is unedited.
- Issues filed: #619, #620, #621, #622.
- issue_609 is rebound to this worktree (koto-f940b700). It carries
  dispatched-by links to both coordinator runs.
- Test-change: the seat agent:issue_609/accept-2 is accepted as entry
  accept-5 (digest 73c599af), and status reads "counted". accept-1 was
  never used.
- Scrutiny, in the session's ledger:
  - Round 1 at `cae09e6`, all on sonnet: completeness fail
    (a447f7105f457b81a), justification fail (aacfa8efa0536f71a), intent pass
    (a418666ce8e55e30f).
  - One blocking_retry was used (1 of 3).
  - Round 2 at `dfcc937`: the re-verifier (sonnet, a9366e10233654a8a)
    passed and closed all three findings.
- Decisions recorded on issue_609, six in all:
  - the bare GET when the memo says declared;
  - the HEADs excluded from the request count;
  - the rewrite of #608's test;
  - the write-path test adaptations (koto session flush is the strict push;
    init's template.json is allowed under the prefix; a LIST, not a GET,
    after the 403);
  - a memo whose read_at is in the future is not trusted;
  - the no-memo case reads the declaration list-first.

## State

issue_609 is at `scrutiny`, and the verdict passes. Its next tick enters
`review`. That panel has never run, so per the merge seat it runs in full:
three seats at `dfcc937`, then QA.

## Open items

1. Tick into `review` and run the full review panel at the code head, then
   QA. One full panel runs at a time.
2. Ask for the verification suite slot before ticking into verification.
3. Delete this file in its own commit before the PR opens, then open the PR.
   The squash file's first line must end with the PR number in parentheses.
4. CI green at the head. Run the measurement link step for entry
   e-20261108T205224Z-845102fed80d with `--pr`.
5. Record the review round with `review-round.sh record --session issue_609`:
   one entry per seat with Seat, Model, Run and Verdict, all passing, at the
   reported head.
6. Send the READY digest. Don't tick past ci_monitor without the
   coordinator's word.
