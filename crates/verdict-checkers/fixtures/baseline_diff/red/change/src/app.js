const express = require('express');
const app = express();

app.get('/login', (req, res) => res.send('login failed: try again'));
app.get('/admin/export', (req, res) => res.send('all users'));

module.exports = app;
