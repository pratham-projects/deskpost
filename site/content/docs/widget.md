---
title: Widget
group: Features
order: 6
description: Let customers file tickets from your own site.
---

# Customer widget

The web app ships a form page for customers. Embed it as:

```
https://desk.example.com/widget?project=<project id>&key=<project api key>
```

Without both parameters the page says it needs them.

```html
<iframe src="https://desk.example.com/widget?project=PROJECT_ID&key=PROJECT_KEY"
        title="Contact support" width="420" height="640" style="border:0"></iframe>
```

`/widget` is deliberately frameable: the shipped `Caddyfile` enforces `frame-ancestors 'none'` for the rest of the UI but leaves this page open, and sets `Referrer-Policy: no-referrer` on it.

## What it does

Submitting creates a ticket through `POST /api/v3/tickets`, a public route. It needs the project API key in `X-Project-Key`, is rate limited, and answers `401` for a missing or wrong key. The client is found or created by lower-cased email within the project, and the ticket gets the project's next number. The API sends the project's enabled `created` auto-reply after commit when the request sets `sendNotification`; the shipped form sets it to `false`, so the form itself sends no confirmation mail. A mail failure never fails ticket creation.

The form collects name, email, subject and message (`subject` 5 to 255 characters, `description` 1 to 20000) and files the ticket at `normal` priority.

## The API key

The key is stored hashed and shown once, when the project is created or the key regenerated (`POST /api/v3/projects/{id}/regenerate-api-key`). The old key stops working immediately. The key is visible to anyone who views the embedding page, so treat it as a public form token, not a credential.
