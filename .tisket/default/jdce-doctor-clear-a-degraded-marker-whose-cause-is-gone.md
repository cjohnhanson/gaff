---
title: 'doctor: clear a degraded marker whose cause is gone'
status: todo
priority: null
assignee: null
due_date: null
labels:
- doctor
- state
depends_on: []
created: 2026-08-16T13:57:16Z
updated: 2026-08-16T13:57:16Z
---

A 0-byte degraded marker from a broken config on 2026-08-14 still shows in doctor a day later, next to config: ok. The next clean parse, or doctor itself, should clear it; otherwise doctor reports a permanent false alarm.
