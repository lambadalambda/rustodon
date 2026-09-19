#!/usr/bin/env python3
"""Bounded real HTTP comparison, not a synthetic Rails oracle.

Only history is excluded (DB aggregate vs Redis retention/activity). Generated
IDs are bijectively paired from actual responses; no status/body expected-value
fixtures stand in for Rails. Full captured tag/error bodies remain in evidence.
"""
import copy
import http.client
from itertools import groupby
import json
from pathlib import Path
import re
import time
from urllib.parse import quote

HOST = 'fixture-v4-6-5.rustodon.invalid'
TARGETS = [('rails', 'hashtag-differential-bd01aca-rails-alice', 3000), ('rust', '127.0.0.1', 18374)]
TOKENS = {
    'owner': 'fixture-bearer-token-v4-6-5',
    'other-owner': 'hashtag-fixture-other-owner',
    'write-accounts': 'hashtag-fixture-write-accounts',
    'write-follows': 'hashtag-fixture-write-follows',
    'read-accounts': 'fixture-bearer-read-accounts-v4-6-5',
    'wrong-scope': 'fixture-bearer-insufficient-v4-6-5',
    'legacy-follow': 'fixture-bearer-follow-v4-6-5',
    'application': 'hashtag-fixture-application',
    'revoked': 'fixture-bearer-revoked-v4-6-5',
    'disabled': 'fixture-bearer-disabled-user-v4-6-5',
    'invalid': 'invalid-task-token',
}


def capture(target, method, path, auth, form):
    side, host, port = target
    assert path.startswith('/api/v1/') and '\n' not in path
    headers = {'Host': HOST, 'X-Forwarded-Proto': 'https', 'Accept': 'application/json'}
    if auth is not None:
        headers['Authorization'] = 'Bearer ' + TOKENS[auth]
    body = None
    if form is not None:
        headers['Content-Type'] = 'application/json'
        body = json.dumps(form).encode()
    conn = http.client.HTTPConnection(host, port, timeout=10)
    try:
        conn.request(method, path, body, headers)
        response = conn.getresponse()
        data = response.read(65537)
        if len(data) > 65536:
            raise ValueError('response exceeds bound')
        result = {'status':response.status, 'content_type':response.getheader('Content-Type', '').split(';')[0], 'body':json.loads(data) if data else None}
        return result
    finally:
        conn.close()


def without_history(value):
    """Validate—not invent—history. Keep all other fields and scalar types."""
    if isinstance(value, list):
        return [without_history(x) for x in value]
    if not isinstance(value, dict):
        return value
    result = copy.deepcopy(value)
    if 'history' in result:
        history = result.pop('history')
        assert isinstance(history, list) and len(history) == 7, history
        for day in history:
            assert set(day) == {'day', 'uses', 'accounts'}, day
            assert all(isinstance(v, str) and re.fullmatch(r'\d+', v) for v in day.values()), day
        days = [int(d['day']) for d in history]
        assert all(days[i] - days[i+1] == 86400 for i in range(6)), days
    return result


def ordered_ties(rows):
    # Rails declares statuses_count DESC, but no tiebreaker. Canonicalize only
    # contiguous equal-count groups, never a distinct-count ordering difference.
    if not all(isinstance(x, dict) and 'statuses_count' in x and 'name' in x for x in rows):
        return rows
    return [x for _, group in groupby(rows, lambda x: x['statuses_count'])
            for x in sorted(group, key=lambda x: x['name'])]


class Comparison:
    def __init__(self, evidence):
        self.evidence = evidence
        self.rows = []
        self.ids = [{}, {}]
        self.next_id = 0

    def pair_ids(self, left, right):
        # Only the undeclared tiebreaker is normalized; actual rows stay intact.
        if isinstance(left, list) and isinstance(right, list):
            for a, b in zip(ordered_ties(left), ordered_ties(right)):
                self.pair_ids(a, b)
        if not isinstance(left, dict) or not isinstance(right, dict):
            return
        if 'name' not in left or left.get('name') != right.get('name'):
            return
        a, b = left.get('id'), right.get('id')
        if a in (None, '') or b in (None, ''):
            return
        assert isinstance(a, str) and isinstance(b, str) and a.isdigit() and b.isdigit()
        # Hashtag IDs and featured relationship IDs belong to different tables.
        kind = 'featured' if 'statuses_count' in left else 'tag'
        keys = [(kind,a), (kind,b)]
        known = [self.ids[i].get(keys[i]) for i in range(2)]
        if any(known):
            assert known[0] == known[1] and all(known), 'unstable or non-bijective IDs'
        else:
            self.next_id += 1
            for i in range(2):
                self.ids[i][keys[i]] = f'{kind}-pair-{self.next_id}'

    def normalized(self, body, side):
        if isinstance(body, list):
            return [self.normalized(x, side) for x in ordered_ties(body)]
        result = without_history(body)
        if isinstance(result, dict) and 'name' in result and result.get('id'):
            kind = 'featured' if 'statuses_count' in result else 'tag'
            result['id'] = self.ids[side].get((kind,result['id']), result['id'])
        return result

    def case(self, label, method, path, auth='owner', form=None):
        paths = [path, path] if isinstance(path, str) else path
        responses = [capture(target, method, paths[i], auth, form) for i, target in enumerate(TARGETS)]
        row = {'case':label, 'method':method, 'paths':paths, 'auth_class':auth, 'form':form, 'rails':responses[0], 'rust':responses[1]}
        # Persist actual responses before normalization, including validation
        # failures. Never store Authorization/cookies or other response headers.
        self.rows.append(row)
        try:
            if responses[0]['status'] == responses[1]['status'] == 200:
                self.pair_ids(responses[0]['body'], responses[1]['body'])
            normalized = [{**r, 'body':self.normalized(r['body'], i)} for i,r in enumerate(responses)]
            row['equal'] = normalized[0] == normalized[1]
        except (AssertionError, TypeError) as error:
            row['equal'] = False
            row['comparison_error'] = str(error)
        self.evidence.write_text(json.dumps(self.rows, indent=2) + '\n')
        print(label, 'MATCH' if row['equal'] else 'MISMATCH', *[r['status'] for r in responses], flush=True)
        return responses


def main():
    evidence = Path('/evidence/differential-responses.json')
    c = Comparison(evidence)
    # Explicit bounded readiness for Rust; Rails health is checked by launcher.
    for _ in range(20):
        try:
            capture(TARGETS[1], 'GET', '/api/v1/tags/FixtureTag', None, None)
            break
        except (OSError, http.client.HTTPException):
            time.sleep(.25)
    else:
        raise RuntimeError('Rust differential endpoint not ready')
    tag = '/api/v1/tags/'
    collection = '/api/v1/featured_tags'
    c.case('lookup-public', 'GET', tag+'FixtureTag', None)
    c.case('lookup-owner', 'GET', tag+'FixtureTag')
    c.case('lookup-case-normalized', 'GET', tag+'FIXTURETAG')
    c.case('lookup-unpersisted', 'GET', tag+'AcceptanceNew')
    c.case('lookup-invalid', 'GET', tag+quote('!!!', safe=''))
    c.case('collection-existing-owner-id', 'GET', collection)
    c.case('delete-other-owner-id', 'DELETE', collection+'/9202', 'other-owner')
    c.case('owner-id-still-present', 'GET', collection)
    c.case('delete-owned-baseline', 'DELETE', collection+'/9202')
    c.case('delete-owned-repeat', 'DELETE', collection+'/9202')
    for action in ('follow','unfollow','feature','unfeature'):
        c.case(action+'-anonymous', 'POST', tag+'FixtureTag/'+action, None)
        c.case(action+'-wrong-scope', 'POST', tag+'FixtureTag/'+action, 'wrong-scope')
    for auth in ('invalid','revoked','disabled','application'):
        c.case('follow-'+auth, 'POST', tag+'FixtureTag/follow', auth)
    c.case('follow-write-follows', 'POST', tag+'AcceptanceNew/follow', 'write-follows')
    c.case('follow-duplicate-normalized', 'POST', tag+'ACCEPTANCENEW/follow', 'write-follows')
    c.case('unfollow-legacy-scope', 'POST', tag+'AcceptanceNew/unfollow', 'legacy-follow')
    c.case('unfollow-absent', 'POST', tag+'AcceptanceNew/unfollow')
    c.case('feature-write-accounts', 'POST', tag+'FixtureTag/feature', 'write-accounts')
    c.case('feature-duplicate-normalized', 'POST', tag+'fixturetag/feature')
    c.case('featured-nonzero-count', 'GET', collection, 'read-accounts')
    c.case('public-featured-read', 'GET', '/api/v1/accounts/116844606259201001/featured_tags', None)
    c.case('unfeature-write-accounts', 'POST', tag+'FixtureTag/unfeature', 'write-accounts')
    c.case('unfeature-absent', 'POST', tag+'FixtureTag/unfeature')
    c.case('collection-anonymous', 'POST', collection, None, {'name':'FixtureTag'})
    c.case('collection-wrong-scope', 'POST', collection, 'write-follows', {'name':'FixtureTag'})
    c.case('collection-read-wrong-scope', 'GET', collection, 'write-accounts')
    c.case('collection-missing-name', 'POST', collection, form={})
    c.case('collection-empty-name', 'POST', collection, form={'name':''})
    c.case('collection-invalid-name', 'POST', collection, form={'name':'!!!'})
    created = c.case('collection-normalized-name', 'POST', collection, 'write-accounts', {'name':' #FixtureTag '})
    c.case('collection-exact-duplicate', 'POST', collection, form={'name':'FixtureTag'})
    c.case('collection-case-duplicate', 'POST', collection, form={'name':'fixturetag'})
    if all(r['status'] == 200 and isinstance(r['body'], dict) and r['body'].get('id') for r in created):
        paths = [collection+'/'+r['body']['id'] for r in created]
        c.case('delete-created-wrong-scope', 'DELETE', paths, 'write-follows')
        c.case('delete-created-other-owner', 'DELETE', paths, 'other-owner')
        c.case('delete-created-owned', 'DELETE', paths, 'write-accounts')
    c.case('empty-before-limit', 'GET', collection)
    for i in range(10):
        c.case('limit-create-'+str(i+1), 'POST', collection, form={'name':f'acceptlimit{i}'})
    c.case('limit-eleventh-collection', 'POST', collection, form={'name':'acceptlimitextra'})
    c.case('limit-eleventh-header', 'POST', tag+'HeaderLimitExtra/feature')
    c.case('limit-existing-header', 'POST', tag+'acceptlimit0/feature')
    # Individual order among equally used tags is not part of this gate; count
    # is derived from actual response, not synthesized into a fixture body.
    final = c.case('limit-final-collection', 'GET', collection)
    counts = [len(r['body']) if isinstance(r['body'], list) else None for r in final]
    bad = [r['case'] for r in c.rows if not r['equal']]
    summary = {'source':'bd01acae2bc4e1b8a75bd95648e216535c790330', 'reference':'1440d55b139e39ec722c2a3db7f60b66cd889048', 'cases':len(c.rows), 'mismatches':bad, 'final_counts':counts, 'history':'actual retained in evidence; excluded from equality (DB aggregation vs Redis retention/activity)', 'peer':'workers disabled, internal task network; AddHashtag/RemoveHashtag delivery deferred', 'result':'PASS' if not bad and counts == [10,10] else 'FAIL'}
    Path('/evidence/differential-summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    if summary['result'] != 'PASS':
        raise SystemExit('actual response mismatch; report before any application fix')


if __name__ == '__main__':
    main()
