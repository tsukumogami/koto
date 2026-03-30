```mermaid
stateDiagram-v2
    direction LR
    [*] --> review
    review --> merge_prep : verdict: approve
    review --> revision : verdict: request-changes
    review --> parked : verdict: defer
    revision --> review : status: revised
    merge_prep --> [*]
    parked --> [*]
```
