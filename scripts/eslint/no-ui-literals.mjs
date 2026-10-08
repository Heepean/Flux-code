const structuralAttributes = new Set([
  'accept',
  'action',
  'aria-describedby',
  'aria-hidden',
  'aria-labelledby',
  'autoComplete',
  'className',
  'cx',
  'cy',
  'd',
  'download',
  'fill',
  'focusable',
  'form',
  'height',
  'href',
  'htmlFor',
  'id',
  'key',
  'method',
  'name',
  'opacity',
  'pattern',
  'preserveAspectRatio',
  'r',
  'rel',
  'role',
  'src',
  'stroke',
  'strokeLinecap',
  'strokeLinejoin',
  'strokeWidth',
  'tabIndex',
  'target',
  'transform',
  'type',
  'viewBox',
  'width',
  'x',
  'xmlns',
  'xmlnsXlink',
  'y',
])

function isTranslationCall(node) {
  if (node?.type !== 'CallExpression') return false
  if (node.callee.type === 'Identifier') return node.callee.name === 't' || node.callee.name === 'translate'
  return node.callee.type === 'MemberExpression'
    && !node.callee.computed
    && node.callee.property.type === 'Identifier'
    && (node.callee.property.name === 't' || node.callee.property.name === 'translate')
}

function containsVisibleString(node) {
  if (!node || typeof node !== 'object') return false
  if (isTranslationCall(node)) return false
  switch (node.type) {
    case 'Literal':
      return typeof node.value === 'string' && node.value.trim().length > 0
    case 'TemplateLiteral':
      return node.quasis.some((part) => part.value.cooked?.trim().length > 0 || part.value.raw.trim().length > 0)
    case 'ConditionalExpression':
      return containsVisibleString(node.consequent) || containsVisibleString(node.alternate)
    case 'LogicalExpression':
      return node.operator === '&&'
        ? containsVisibleString(node.right)
        : containsVisibleString(node.left) || containsVisibleString(node.right)
    case 'BinaryExpression':
      return node.operator === '+'
        && (containsVisibleString(node.left) || containsVisibleString(node.right))
    case 'SequenceExpression':
      return containsVisibleString(node.expressions.at(-1))
    case 'ArrayExpression':
      return node.elements.some(containsVisibleString)
    case 'CallExpression':
      return node.arguments.some(containsVisibleString)
    case 'ChainExpression':
    case 'AwaitExpression':
    case 'ParenthesizedExpression':
    case 'TSAsExpression':
    case 'TSNonNullExpression':
    case 'TSTypeAssertion':
      return containsVisibleString(node.expression)
    case 'JSXExpressionContainer':
      return containsVisibleString(node.expression)
    default:
      return false
  }
}

function reportLiteral(context, node) {
  context.report({ node, messageId: 'noVisibleLiteral' })
}

export default {
  meta: {
    type: 'problem',
    docs: { description: 'Require translations for user-visible JSX text and string attributes.' },
    messages: { noVisibleLiteral: 'Move user-visible UI text into a locale resource and render it through a translation call.' },
    schema: [],
  },
  create(context) {
    return {
      JSXText(node) {
        if (node.value.trim().length > 0) reportLiteral(context, node)
      },
      JSXAttribute(node) {
        if (node.name.type !== 'JSXIdentifier') return
        const attributeName = node.name.name
        if (structuralAttributes.has(attributeName) || attributeName.startsWith('data-')) return
        if (containsVisibleString(node.value)) reportLiteral(context, node.value)
      },
      JSXExpressionContainer(node) {
        if (node.parent.type === 'JSXAttribute') return
        if (containsVisibleString(node.expression)) reportLiteral(context, node.expression)
      },
    }
  },
}