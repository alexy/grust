#!/usr/bin/env python3
"""Freeze Helix SDK observations plus client and source-built server provenance.

This is a transport bundle, not an independent publication receipt.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


common = module('sdk_bundle', 'bundle-sdk.py')
audit = module('helix_audit', 'validate-helix-sdk.py')
SERVER_SOURCE = '0ef3cee0faf28bb81072fb149b982dcdb166d60a'
SERVER_RECIPE = 'c30ce29fe4145a2a3d5fbe58eee892c7451e65542281469645dae2e10337b504'
SERVER_LOG = '3c93df19ee175ce801149056165cf140feed9e6b060839854a1b2cc3a0e37199'


def export(directory, output, client_build, server_build, server_recipe):
    receipt_bytes = common.read(server_build / 'build-receipt.json')
    receipt = json.loads(receipt_bytes)
    expected = dict(schema='grust-helix-sdk-server-build-v1', publication_qualified=False,
                    source_repository='https://github.com/HelixDB/helix-db',
                    source_revision=SERVER_SOURCE, source_clean_before_and_after=True,
                    server_package_version='0.1.0', sdk_version_target='3.0.0',
                    platform='linux/arm64', image_id=audit.SERVER, build_exit=0,
                    recipe_sha256=SERVER_RECIPE, build_log_sha256=SERVER_LOG)
    for key, value in expected.items():
        audit.require(type(receipt.get(key)) is type(value) and receipt[key] == value,
                      f'server build {key} differs')
    recipe = common.read(server_recipe)
    log = common.read(server_build / 'command.log')
    audit.require(hashlib.sha256(recipe).hexdigest() == SERVER_RECIPE,
                  'server recipe differs')
    audit.require(hashlib.sha256(log).hexdigest() == SERVER_LOG,
                  'server build log differs')
    return common.export(directory, output, client_build, auditor=audit, backend='helix-sdk',
                         additional_files={'server-build/build-receipt.json': receipt_bytes,
                                           'server-build/Dockerfile': recipe,
                                           'server-build/build.log': log})


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--client-build', required=True, type=Path)
    parser.add_argument('--server-build', required=True, type=Path)
    parser.add_argument('--server-recipe', type=Path, default=ROOT / 'Dockerfile.helix-sdk-server')
    args = parser.parse_args()
    print(json.dumps(export(args.directory, args.output, args.client_build,
                            args.server_build, args.server_recipe)))
