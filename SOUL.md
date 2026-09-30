# pigeon soul

This file holds the rules that stay true for pigeon's whole life. Keep it
short, and change it only if the project's identity changes.

## Mission

pigeon keeps a trusted group's files in sync, peer to peer, giving every file
exactly one owner, whose machines alone write it.

## Design rules

1. **One writer per file.** Only the machines of a file's owner write it;
   every other change is a request that one of those machines applies. This
   is what keeps people from ever conflicting, and no feature may break it.
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
