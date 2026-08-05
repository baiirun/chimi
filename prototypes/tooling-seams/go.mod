module chimi-tooling-seams-prototype

go 1.24

require (
	chimi-lossless-parser-prototype v0.0.0
	chimi-semantic-lowering-prototype v0.0.0
)

replace chimi-lossless-parser-prototype => ../lossless-parser

replace chimi-semantic-lowering-prototype => ../semantic-lowering
