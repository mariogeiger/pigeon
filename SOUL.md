# pigeon soul

This file holds the rules that stay true for pigeon's whole life. Keep it
short, and change it only if the project's identity changes.

## Mission

pigeon keeps a trusted group's files in sync, peer to peer: every member may
change every file, simple rules publish what is clearly theirs to change, and
the group decides the rest.

## Design rules

1. **Rules publish, the group decides, nothing is lost.** A machine
   publishes by itself only its member's changes to their own files and new
   files where no one owns the path; every other change, and the losing
   side of concurrent ones, becomes a suggestion that anyone validates or
   discards, the first decision final. Every version stays in the history,
   and no feature may break this.
2. **Trust the group, guard against mistakes.** Members are trusted; safety
   comes from visibility, history, and undo, never from walls between people.
3. **Rust, reusing before writing.** pigeon is written in Rust and builds on
   existing libraries and tools that integrate cleanly with it; new code is
   reserved for what makes pigeon pigeon.
4. **Choose the simplest sufficient design.** Extend one general mechanism
   rather than add a special case; complexity must pay for a requirement.
5. **Fix problems at the source, and say so.** Reject a bad state where it is
   created instead of repairing it downstream, and tell the person why and
   how to fix it.
6. **Earn claims with tests.** Every guarantee, starting with rule 1, has a
   test that fails when the guarantee breaks.
7. **Name things by what they do.** A name states the operation on the data,
   never the context that calls it.
8. **Keep each file bounded.** Give each file one mission, state it in its
   header, and split the file before it reaches 1000 lines.
9. **Write in English.** The code, the documentation, and everything pigeon
   says to the people who use it are in English.
10. **The web and the command line do the same.** Everything doable in the
    web interface is doable on the command line, and the other way round,
    with as little code as possible written twice.
