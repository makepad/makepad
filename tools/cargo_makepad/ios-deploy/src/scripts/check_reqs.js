#!/usr/bin/env node

var util = require('util');
var child_process = require('child_process');

var XCODEBUILD_NOT_FOUND_MESSAGE = 'Please install Xcode from the Mac App Store.';
var TOOL = 'xcodebuild';

var xcode_version = child_process.spawn(TOOL, ['-version']);

xcode_version.stderr.on('data', function (data) {
	console.log('stderr: ' + data);
});

xcode_version.on('error', function (err) {
	console.log(util.format('Tool %s was not found. %s', TOOL, XCODEBUILD_NOT_FOUND_MESSAGE));
});

