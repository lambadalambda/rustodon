#!/usr/bin/env python3
"""Offline guard for the bounded hashtag acceptance adapter (not browser evidence)."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class AdapterTests(unittest.TestCase):
    def test_isolated_no_workers(self):
        spec = importlib.util.spec_from_file_location('prepare', ROOT / 'tools/hashtag-controls-browser/prepare.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            module.prepare(ROOT, Path(directory))
            run = (Path(directory) / 'run.sh').read_text()
            setup = (Path(directory) / 'setup.sh').read_text()
            self.assertIn('bd01acae2bc4e1b8a75bd95648e216535c790330', run)
            self.assertIn('tools/hashtag-controls-browser/ui.py', run)
            self.assertNotIn('run pull ', run)
            self.assertNotIn('run source ', run)
            self.assertIn('for mode in web;', setup)
            self.assertIn('network create --internal', setup)
            self.assertIn('browser_writer', setup)
            self.assertNotIn('RUSTODON_TEST_PEER_ORIGINS=', setup)
            self.assertIn('seed.sql', setup)
            self.assertIn('remaining-task-containers', (Path(directory) / 'cleanup.sh').read_text())

    def test_typed_mode_waits_for_actual_instance_metadata(self):
        spec = importlib.util.spec_from_file_location('prepare', ROOT / 'tools/hashtag-controls-browser/prepare.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            module.prepare(ROOT, Path(directory), mode='typed')
            run = (Path(directory) / 'run.sh').read_text()
            self.assertIn('featured-typed-d17bec9', run)
            self.assertIn('ui.py typed', run)
            self.assertIn('d17bec9ceaea8791323e630d5fd5bd10dc2bd67d', run)
        controller = (ROOT / 'tools/hashtag-controls-browser/ui.py').read_text()
        self.assertIn('max_featured_tags===10', controller)
        self.assertIn("'fill', 'input[type=search]'", controller)
        self.assertIn("'Add #' + name", controller)
        self.assertIn('range(10)', controller)

    def test_differential_reuses_real_rails_and_isolates(self):
        spec = importlib.util.spec_from_file_location('prepare', ROOT / 'tools/hashtag-controls-browser/prepare.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            module.prepare(ROOT, out, mode='differential')
            run = (out / 'run.sh').read_text()
            rails = (out / 'rails-functions.sh').read_text()
            self.assertIn('differential.sh', run)
            self.assertNotIn('run browser ', run)
            self.assertIn('hashtag-differential-bd01aca', run)
            self.assertIn('696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf', rails)
            self.assertIn('bundle exec puma -C config/puma.rb', rails)
            self.assertNotIn('--publish ', rails)
            self.assertIn('--memory=2g', rails)
            self.assertIn('--pids-limit=256', rails)
            self.assertIn('rails redis', (out / 'cleanup.sh').read_text())

    def test_controller_profile_reload_and_boolean_waits(self):
        import ast
        source = (ROOT / 'tools/hashtag-controls-browser/ui.py').read_text()
        tree = ast.parse(source)
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == 'wait':
                if node.args and isinstance(node.args[0], ast.Constant):
                    code = node.args[0].value
                    self.assertFalse(code.startswith('document.querySelector(') and '.innerText.includes(' not in code, 'CDP waits must return boolean, not DOM objects')
        tail = source.split("record('profile-remove')", 1)[1].split("def editor(", 1)[0]
        self.assertIn('r.path===\"/api/v1/profile\"', tail)
        self.assertNotIn('r.path===\"/api/v1/featured_tags\"', tail)
        self.assertIn("button('Delete FixtureTag')", source)
        self.assertNotIn('fetch(', source)
        self.assertNotIn('dispatch(', source)

    def test_normalization_is_narrow_and_ids_are_bijective(self):
        spec = importlib.util.spec_from_file_location('diff', ROOT / 'tools/hashtag-controls-browser/differential.py')
        m = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(m)
        history = [{'day':str(1789776000-i*86400), 'uses':'2', 'accounts':'1'} for i in range(7)]
        value = {'id':'1', 'name':'Tag', 'history':history, 'following':False, 'extra':None}
        self.assertEqual(m.without_history(value), {k:v for k,v in value.items() if k != 'history'})
        self.assertIn('history', value)
        with self.assertRaises(AssertionError):
            m.without_history({**value, 'history':[]})
        c = m.Comparison(Path('/unused'))
        c.pair_ids(value, {**value, 'id':'42'})
        self.assertEqual(c.normalized(value, 0), c.normalized({**value, 'id':'42'}, 1))
        self.assertNotEqual(c.normalized(value, 0), c.normalized({**value, 'id':'42', 'following':True}, 1))
        with self.assertRaises(AssertionError):
            c.pair_ids(value, {**value, 'id':'43'})
        rows = [{'name':'b', 'statuses_count':'2'}, {'name':'a', 'statuses_count':'2'}, {'name':'c', 'statuses_count':'3'}]
        self.assertEqual([r['name'] for r in m.ordered_ties(rows)], ['a','b','c'])
        self.assertEqual(m.ordered_ties([{'name':'a', 'statuses_count':2}, {'name':'b','statuses_count':'2'}])[0]['statuses_count'], 2)


if __name__ == '__main__':
    unittest.main()
