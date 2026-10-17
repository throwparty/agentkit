---
description: Toggle the editor's file-system and terminal tools for this connection
parameters:
  - category
  - enabled
---
Toggles which categories of the editor's advertised capabilities are reflected to the model for the rest of the connection. Usage: `/editor fs <true|false>`, `/editor terminal <true|false>`. The harness intercepts this command and never turns it into a model prompt.
