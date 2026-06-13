#!/usr/bin/env python3
"""Regenerate contract/canonical-operations.json from the canonical OpenAPI spec.

The CLI's contract test (tests/contract_projection.rs) asserts that the set of
operations the CLI actually implements (contract/implemented-operations.json) is
an honest subset of the operations the live API publishes. The reference set is
derived here from the canonical spec so the test can never be a tautology that
compares two copies of the same hand-maintained file.

Usage:
    python3 scripts/gen-canonical-operations.py [path/to/dairo.openapi.json]

If no path is given it defaults to the in-repo copy referenced by the backend.
The output is committed so the test stays deterministic and offline.
"""

from __future__ import annotations

import json
import sys
from collections import OrderedDict
from pathlib import Path

DEFAULT_SPEC = (
    Path(__file__).resolve().parents[2]
    / "dairo-backend/backend/dairo-api/openapi/dairo.openapi.json"
)
OUTPUT = Path(__file__).resolve().parents[1] / "contract/canonical-operations.json"
HTTP_METHODS = ("get", "post", "put", "patch", "delete")


def extract(spec: dict) -> list[dict]:
    operations: list[dict] = []
    for path, methods in spec["paths"].items():
        for method, op in methods.items():
            if method.lower() not in HTTP_METHODS:
                continue
            parameters = []
            for param in op.get("parameters", []):
                entry: dict[str, str] = {}
                if "in" in param:
                    entry["in"] = param["in"]
                if "name" in param:
                    entry["name"] = param["name"]
                parameters.append(entry)
            operations.append(
                {
                    "method": method.upper(),
                    "operationId": op.get("operationId"),
                    "parameters": parameters,
                    "path": path,
                    "scopes": op.get("x-dairo-scopes") or [],
                    "contractSource": "openapi",
                }
            )
    operations.sort(key=lambda o: (o["path"], o["method"]))
    return operations


def main() -> int:
    spec_path = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_SPEC
    spec = json.loads(spec_path.read_text())
    operations = extract(spec)

    document = OrderedDict()
    document["name"] = "dairo-public-api-canonical-openapi-contract"
    document["description"] = (
        "Derived from the canonical dairo.openapi.json. "
        "Regenerate via scripts/gen-canonical-operations.py; do not hand-edit."
    )
    document["openapiVersion"] = spec.get("openapi")
    document["specVersion"] = spec.get("info", {}).get("version")
    document["operationCount"] = len(operations)
    document["operations"] = operations

    OUTPUT.write_text(json.dumps(document, indent=2) + "\n")
    print(f"wrote {OUTPUT} ({len(operations)} operations) from {spec_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
