Credentials are now fully redacted in tool responses and CLI output; no part of a secret is shown.

Before this change a masked secret kept its last four characters, including bearer tokens and API keys, in tool responses and CLI output, so a caller who could read the response got part of the credential. Every masked secret is now the fixed `[REDACTED]` literal.
