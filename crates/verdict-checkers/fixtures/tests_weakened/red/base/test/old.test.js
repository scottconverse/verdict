const { old } = require('../src/old');

test('old', () => {
  expect(old()).toBe('old');
});
