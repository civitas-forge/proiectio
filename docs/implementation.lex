Implementation Guidelines

Proiectio is *entirely* about side effects. This poses a challenge, on testability and maintainability.

If not properly managed, the code base will become untestable, hence unreliable, hence bug ridden as time goes on.

1. Pure Core Engine

    Separate the file system writes and reads from the logic. That is, before ever writing, we will process a pure data structure representing the desired state, and then apply it to the file system.

    This allows us to contain all side effects in a single place, making the codebase easier to test and maintain. This is true for both writing and removing, dry runs, that is, the engine must enforce that.

2. Testing Helpers

    We should have helpers that allow fast declaration of trees and various test scenarios.

    While the core engine is to be pure, and the full flow must leverage dependency injection, it does matter that we also test the file system changes, that is the apply engine itself.

    In order to avoid data loss or complicated ordering bugs, we should have cnetralized setup and teardown that guarantees an isolated and clean working directory for tests.

3. Centralized Path Resolution

    It's critical that path resolution be centralized, both to ensure security and to avoid path resolution bugs, especially between different oses.

    Do research and make use of well established crates that can take a significant amount of work and risk off our table. [1]

4. Standout

    The application is to be built on top of the standout framework, which enforces a hard split between a pure rust api and data types, including returns, and the cli interface, which maps perfectly with the project's design.

    The corolary is we will work on the pure library first, and only tackle the cli interface after that. See the /standout skill for best practices.

5. Error Handling

    While it's desirable to , during the pure rust core validate and exit early for errors, this project's nature cannot guarantee that no other runtime errors will occur . For example, mid injection it's possible for the disk to run out of space, or permissions change, or entire trees to be deleted.

    The message here is not about preventing these, as that's not possible. But ensuring that we never swallow these errors is crucial, it's fine to present a friendlier error message than rust would, but that cannot change the nature of the error nor semantics. As much as possible the exit codes from these match the file system's ones.

    And finally, under no circumstances should we continue after errors, or to try to recover or rollback. Errors blow up as soon as they happen, and we don't cleanup after ourselves.

Notes:

[1] Some crates worth looking into:
    Popular Path-Handling Cratespath-absolutize: Extends standard paths to resolve absolute paths and clean up dot components (. or ..) without strictly needing full filesystem canonicalization.normpath: Offers more predictable, platform-consistent path normalization as a reliable alternative to std::fs::canonicalize.relative-path: Provides portable, strictly relative path types that function independently of the host operating system's native path rules.strict-path: Focuses on security by validating untrusted path strings to prevent directory traversal and symlink escape vulnerabilities.
