# Public profile: data-tools, version 3

Profile identifier: `plenora-data-tools-profile-v3`

Normative target catalog:
[`data-tools-v3.json`](../catalogs/data-tools-v3.json), with the exact
[`data kernel registry v2`](../catalogs/data-kernels-v2.json).

It supersedes [profile version 2](data-tools-v2.md) as the current target
([decision 0015](../decisions/0015-intentional-absences-and-planned-surfaces.md)).
Its catalog lists the operation identities of version 2, unchanged, under
Public Catalogs 2.0 (`plenora-public-catalog-v2`); this profile adds the
intentional absences and states the runtime as conditional under that
contract. Versions 2 and 1 ([decision 0007](../decisions/0007-data-tools-v2.md))
remain available identities: their catalogs, registries and plan formats
keep their meaning.

## Applicable contracts

- [Public Surfaces 1.0](../specs/surfaces/PUBLIC-SURFACES-1.0.md)
- [Capability Discovery 2.0](../specs/capabilities/CAPABILITY-DISCOVERY-2.0.md)
- [Typed Errors 1.0](../specs/errors/ERRORS-1.0.md)
- [Public Security 1.0](../specs/security/PUBLIC-SECURITY-1.0.md)
- [Arrow Interchange 1.0](../specs/data/ARROW-INTERCHANGE-1.0.md)
- [Data Plan 1.0](../specs/data/DATA-PLAN-1.0.md)
- [Row Diagnostics 1.0](../specs/diagnostics/ROW-DIAGNOSTICS-1.0.md)
- [CLI 2.0](../specs/cli/CLI-2.0.md)
- [Python SDK 1.0](../specs/sdk/PYTHON-SDK-1.0.md), when exposed
- [Public Catalogs 2.0](../specs/catalogs/PUBLIC-CATALOGS-2.0.md), for the
  catalog of this profile
- [Runtime Binding 2.0](../specs/runtime/RUNTIME-BINDING-2.0.md), when exposed
- [Surface Bindings 1.0](../specs/surfaces/SURFACE-BINDINGS-1.0.md)
- [Composition 1.0](../specs/composition/COMPOSITION-1.0.md)

[Plan Budget 1.0](../specs/data/PLAN-BUDGET-1.0.md) does not apply to this
profile: its plan formats `4`, `5` and `6` belong to profile version 1.

## Public purpose

The component exposes versioned table and geospatial transformations over
declared tabular contracts.

## Required operation families

Every kernel in the registry MUST retain its stable identifier. Its `version`
is the version of its observable semantics: a change of output, defaults,
accepted configuration or error axes raises it.

**DT-001** — The result of `data.catalog` version 2
(`plenora-data-catalog-result-v2`) is a JSON object with:

- `registry`: the kernel registry the artifact implements, as
  [`data-kernels-v2.json`](../catalogs/data-kernels-v2.json) shapes it
  (`plenora-operation-registry-v1`), listing every kernel the artifact can
  execute at the registry's `id` and `version`;
- `kernels`: one component-owned descriptor per registered kernel, with at
  least `id`, `version` (equal to the registry's), `family` and `status`
  (`available` or `unavailable`); an `unavailable` kernel carries a `reason`
  and is absent from `registry`;
- `plan_format`: `plenora-data-plan-v1`.

A kernel the artifact cannot execute is never reported available, and a plan
using it fails with `unsupported` (DPLAN-006). Kernel status is distinct from
the status of the `data.catalog` operation in Capability Discovery 2.0.

The public surface MUST also make these functions discoverable:

- catalog inspection;
- plan validation;
- transformation execution.

Catalog attributes expose externally relevant operation properties such as
input arity, result shape, determinism, required backend and CRS requirement.
They do not expose executor internals.

## Public surfaces

- Rust API: required.
- CLI: required and governed by CLI 2.0.
- Python SDK: optional; when published it is governed by Python SDK 1.0 and
  binds all four operations in both API modes.
- Runtime: conditional. `data.catalog` version 2, `data.describe` version 1
  and `data.validate` version 2 list it. `data.run` version 2 does not: its
  result names any number of outputs, and Runtime Binding 2.0 carries one
  payload per response. `data.run` version 3 is the named-output runtime
  representation ([«Runtime execution»](#runtime-execution-datarun-version-3)):
  sources and sinks are artifacts, and its one response is the manifest of
  the published outputs. It is conditional and lists the Rust and runtime
  surfaces only. An artifact without a runtime binding omits `runtime` from
  its capability document, and the runtime entries of `data.catalog`
  version 2, `data.describe` version 1, `data.validate` version 2 and
  `data.run` version 3 are planned for it (CAT-001, CAT-002).

## Surfaces intentionally absent

These gaps in the catalog are decisions, not missing work. Public Catalogs
2.0 CAT-003 keeps them: none of these surfaces is ever added to these
operation identities.

**DT-ABS-001** — `data.run` version 3 is bound to neither the CLI nor the
Python SDK, because the current contracts define no artifact resolver for
those surfaces: version 3 resolves references only through the resolver the
final application provides (RT-015, DT-RUN-002). The local case is already
served by version 2 with the same semantics (DT-RUN-004): `--input NAME=PATH`
and `--output NAME=PATH` on the CLI, tables or paths in Python. A binding of
version 3 on those surfaces needs a decision that defines their resolver and
its reference grammar.

**DT-ABS-002** — `data.run` version 2 is not bound to the runtime, because
the current contracts define no runtime form for its result: it names any
number of outputs, and Runtime Binding 2.0 carries one payload per response.
Version 3 is its runtime representation.

## Interchange

Tabular and geospatial operations accept and return Arrow according to Arrow
Interchange 1.0. Operation-specific contracts define logical columns and
operation parameters.

Parquet is an extension content type declared in the operation attributes
(`extension_content_types`), outside the Arrow interchange contract: a
consumer that does not recognize it MUST NOT send it.

`describe`, `validate` and `run` declare `bounded_materialization`: tables are
materialized within the plan memory budget (ARROW-011).

Row-scoped validation or mapping failures use
`plenora-row-diagnostics-v1` when diagnostics are advertised.

## Arrow boundary of version 2 operations

These rules bind the version 2 operations of this profile. They decide cases
Arrow Interchange 1.0 and the vocabulary leave to the operation contract; they
do not change those contracts for other components.

**DT-ARROW-001** — Within one output schema no identifier appears on two
fields, and identifiers declared by different plan inputs are independent:
the same number may name different fields. A field whose identifier is
carried by another field of the output (a duplicated column, a self-join), or
is declared by more than one plan input, receives a new identifier, as a newly
derived field; the component MUST NOT keep the identifier on one of those
fields by choice. The consumer loses that field's identity and never finds
another field under it.

**DT-ARROW-002** — An operation that concatenates the rows of several inputs
into one output field (`table.concat`, `table.concat_by_name`,
`table.union_distinct`) takes that field's metadata, identifier included,
from the corresponding field of the first input. DT-ARROW-001 then applies
to the published output: when the inputs declare the same identifier for the
concatenated column, the published field receives a new identifier.

**DT-ARROW-003** — An input schema whose `geoarrow.wkb` field omits
vocabulary keys that producers MUST declare is accepted, and each missing key
is read without asserting more than the field carries: `encoding` as the
extension name reads (`wkb`), `dimensions` `unknown`, `types_declaration`
`unresolved`, `spatial_semantics` `geometry` (the GeoArrow default),
`precision` `float64` (the WKB coordinate width), and the CRS state from the
CRS representations present (`missing` when there are none). Contradictory
keys still fail (ARROW-006), and every output geometry field declares every
key.

**DT-ARROW-004** — A geometry operation rejects, before computing, a field
whose `spatial_semantics` is `geography` or whose GeoArrow extension metadata
declares non-planar `edges` (category `unsupported`), and a field whose
`axis_order` puts north first (`lat_lon`, `northing_easting`; category
`crs`). An operation that only passes the field through carries it unchanged
(ARROW-008).

## Outputs and side effects

`data.run` returns the outputs a plan names, in the order it names them
(`plenora-data-execution-result-v2`). A surface that takes output paths
writes each output to its local file, which `side_effect: local` declares; a
surface that returns the tables in memory writes nothing outside the process.

Every surface of `data.run` version 2 names each output (CLI
`--output NAME=OUTPUT.arrow`, Rust and Python mappings), so each accepts any
number of outputs; on the CLI the unnamed `--output OUTPUT.arrow` is accepted
for a plan with exactly one output.

## Runtime execution (`data.run` version 3)

`data.run` version 3 executes the same plans as version 2 where the caller
cannot hand tables or paths across the boundary. The request
(`plenora-data-execution-input-v3`,
[`data-execution-input-v3.schema.json`](../schemas/data-execution-input-v3.schema.json))
carries the plan, one artifact source per plan input and one artifact sink
per plan output. The result (`plenora-data-execution-result-v3`,
[`data-execution-result-v3.schema.json`](../schemas/data-execution-result-v3.schema.json))
is the manifest of the published artifacts. Both are JSON; the tables
travel only as artifacts, Arrow IPC under Arrow Interchange 1.0 or Parquet as
an extension (`artifact_content_types`, `extension_content_types`).

**DT-RUN-001** — The source names are exactly the plan `inputs` and the sink
names exactly the plan `outputs`. A missing, extra or unknown name fails
before any source is read (`invalid_configuration`, as in version 2, phase
`validate`, `remote_effect: none`).

**DT-RUN-002** — Every source and sink carries an opaque artifact reference
(RT-013), never a private local path; two sinks never share one reference,
and the resolver rejects, before the plan executes, two sinks that resolve
to the same artifact under different references (`invalid_configuration`,
`remote_effect: none`).
The schema rejects the spellings that are paths on their face (`file:`,
`.` and `..` segments, percent-encoded dots, whitespace), but a reference
is opaque by how it is resolved, not by its spelling: the component hands
it unchanged to the resolver the final application provides (RT-015) and
never decodes, normalizes or joins it to a path; the resolver maps it only
within the namespaces it authorizes, and never as a path relative to the
process. A reference that does not resolve, or that the resolver does not
authorize, fails before the plan executes (`not_found` or `authorization`,
`remote_effect: none`). Resolving a source reads it; resolving a sink
creates nothing before publication (DT-RUN-005).

**DT-RUN-003** — A source is read as the content type it declares. When it
declares `expected` size or SHA-256, the bytes read are checked against them
before the plan executes; a mismatch fails (`data_mapping`, phase `read`,
`remote_effect: none`).

**DT-RUN-004** — The plan executes with the semantics of `data.run` version
2 on the same plan and the same input tables: the same kernels, limits,
budget, Arrow rules (DT-ARROW-001 to DT-ARROW-004) and typed errors. The
tables a version 3 sink receives are those version 2 returns.

**DT-RUN-005** — No sink is written before every output is computed and
encoded: a failure while reading, executing or encoding publishes nothing
(`remote_effect: none`). On success the result lists every plan output in
plan order, with its sink reference, rows, columns and the artifact
published: content type, exact byte size and the SHA-256 calculated over the
published bytes, followed by the per-step counts of version 2 (`rows_in`
lists the rows of each step input, in step order).

**DT-RUN-006** — Sinks are published one at a time, in plan output order.
`overwrite: false` is enforced by the sink atomically with the publication
(create only if absent); a resolver that cannot do so rejects such a sink
before the plan executes (`unsupported`, `remote_effect: none`). A sink
with `overwrite: false` whose artifact already exists fails (`conflict`); a
sink with `overwrite: true` replaces it. The outcome of a failed
publication:

- not proven: `unknown`, with retry `never`, `quarantine` or
  `requires_recovery` (ERR-006), whatever was published before it
  (ERR-004: uncertainty prevails);
- proven to have written nothing, with no earlier publication: `none`;
- proven to have written part of its artifact (the first sink included,
  an `overwrite: true` replacement included), or proven to have written
  nothing after at least one successful earlier publication: `partial`,
  with retry `never`, `quarantine` or `requires_recovery`.

No result is returned unless every sink was published.

**DT-RUN-007** — Deadline and cancellation behave as in version 2 until
publication starts; after it starts they follow DT-RUN-006. Idempotency keys
are not accepted.

**DT-RUN-008** — Neither the result nor an error carries what a reference
resolved to, a local path or a row value. Plan names (inputs, steps,
outputs) are the caller's data, not locations, whatever their spelling: a
result returns them as given, and an error may name, as given, the plan
input, step or output it concerns. Any other path or reference spelling in
an error is a location.

Conformance: the vectors fix the payloads; the ordering guarantees
(DT-RUN-001, DT-RUN-003, DT-RUN-005, DT-RUN-006) are demonstrated by an
adopter with an instrumented resolver that records reads, publications and
their order and injects failures at each phase: before any read, on a
digest mismatch, during encoding, at the first and at a later sink, and with
an unproven publication outcome.

## External behavior

Unavailable kernels, unsupported CRS states and incompatible schemas fail
closed with typed errors. A catalog entry MUST NOT claim availability when the
released artifact cannot execute it.

## Plan identity

[Data Plan 1.0](../specs/data/DATA-PLAN-1.0.md) defines no plan hash and no
canonical form (DPLAN-012), and no migration from plan formats `4`, `5` or `6`
(DPLAN-013).

## Not specified here

This profile does not prescribe kernel implementation, scheduling, memory
accounting or cancellation-token design.
