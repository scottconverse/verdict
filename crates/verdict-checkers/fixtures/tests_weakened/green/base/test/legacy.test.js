const { legacy } = require('../src/legacy');

test('legacy', () => {
  expect(legacy()).toBe('legacy');
});
