---
title: a handler killed on the read deadline loses the stdout it already wrote
status: todo
priority: null
assignee: null
due_date: null
labels: []
depends_on: []
created: 2026-08-16T21:02:18Z
updated: 2026-08-16T21:02:18Z
---

## Problem

A handler that misses the read deadline is killed and its collected stdout is dropped (`src/handler.rs:557-568`), while one killed on the flush budget keeps what it produced (`:574-598`). Found by QA on 2026-08-16.

## Fix

Inject what arrived before the deadline in both paths, marked as truncated.
