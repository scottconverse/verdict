const { a } = require('../src/app');

test.only('a', () => {
  expect(a()).toBe('a-b');
});
