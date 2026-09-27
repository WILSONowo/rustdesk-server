# 1.1.16-beta1 release baseline

The 2026-09-27 release publishes the user-accepted `1.1.16-beta1` implementation.
The existing tag remains at `65ceee0276c270adea152520a524d14b36605098`.

Before this release, the fork's master pointed to
`a7736be5e40f85bfc141120dce587e836e5d4b80`, which included 12 upstream commits
after the tested 1.1.16 baseline, including 1.1.17 version and dependency changes.
Those changes have not been accepted for this release.

With the repository owner's explicit approval, the release branch records that
master as a merge parent using the `ours` merge strategy. This preserves the full
history while retaining the tested 1.1.16 implementation and submodule revision.
It does not claim that the 1.1.17 changes have been incorporated into this release.

When adopting upstream 1.1.17 later, do not assume that a normal merge alone will
restore the omitted changes: Git already sees those commits in the history.
Review and apply the diff from upstream tag `1.1.16` to the selected new upstream
baseline, port the API handshake patch, and run the tests and client acceptance
again. Start the new upstream series at `1.1.17-beta1` after acceptance.

Only documentation and merge history differ between the accepted beta1 tag and
this release merge. GitHub Release is marked as a regular release, not prerelease.
Publishing does not deploy or restart any server.
