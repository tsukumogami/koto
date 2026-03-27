# Decision 10: How content ownership extends the SessionBackend trait

## Question

How does content ownership extend the existing SessionBackend trait? Where do content operations live in the type system?

## Chosen: Option 2 -- Separate ContextStore trait

**Confidence: high**

## Rationale

The existing `SessionBackend` trait has five methods that all deal with session lifecycle: creating directories, checking existence, cleanup, and listing. Content operations (add, get, exists, list, remove) are a different concern -- they operate *within* an existing session on named keys, not on sessions themselves.

A separate `ContextStore` trait provides the cleanest boundary:

- **Cohesion within each trait.** `SessionBackend` stays focused on "does this session exist and where is it?" while `ContextStore` owns "what content lives inside this session?" Each trait has a single reason to change.
- **Future backend flexibility.** Cloud and git backends may store session metadata differently from content blobs. A cloud backend might use S3 for content but DynamoDB for session records. Separate traits let each backend compose storage strategies independently rather than forcing both concerns into one interface.
- **Implementation simplicity.** `LocalBackend` implements both traits. Callers that need content access accept `&dyn ContextStore`. No wrapper types, no accessor methods, no indirection. The existing `&dyn SessionBackend` call sites don't change at all.
- **No breaking change cost.** Since koto controls all implementations, adding a second trait that `LocalBackend` implements is just as cheap as adding methods to the existing trait. There are no external consumers to coordinate with.

The composition approach (Option 3) adds a layer of indirection (`fn content_store(&self) -> &dyn ContextStore`) that doesn't earn its keep in a codebase with one backend today. It makes sense when backends need to swap content store implementations at runtime, but that's not a current requirement. Option 1 (adding methods directly) conflates two responsibilities and makes the trait harder to reason about as both concerns grow.

## Assumptions

- Content operations will grow beyond the initial five methods (e.g., size limits, metadata, bulk operations), making trait separation more valuable over time.
- CLI handlers that need content access can receive `&dyn ContextStore` alongside or instead of `&dyn SessionBackend` without major refactoring (the dispatch already threads trait objects).
- `LocalBackend` implementing both traits is sufficient; no need for separate structs per concern at the local-filesystem level.

## Rejected

### Option 1: Add methods to SessionBackend

Would balloon the trait from 5 to 10+ methods mixing two unrelated concerns. As content operations grow, the trait becomes a grab-bag. Harder to test in isolation -- mocking session lifecycle shouldn't require stubbing content methods and vice versa.

### Option 3: Composition via ContentStore field

Adds runtime indirection (`fn content_store(&self) -> &dyn ContextStore`) without a concrete benefit today. The accessor pattern makes sense when you need to swap the content store independently of the backend, but koto's current architecture doesn't need that. If it does later, migrating from Option 2 to Option 3 is a one-method addition to `SessionBackend`, not a rewrite.
