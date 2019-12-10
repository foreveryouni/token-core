import { id, toHaveText, label } from '../../../utils.js'

export default async function (params) {
  const { chainType, mnemonic, password, address, network, segWit } = params
  // go to Mnemonic screen
  await id('Mnemonic').tap()

  await id('input-mnemonic').tap()
  await id('input-mnemonic').replaceText(mnemonic)
  // await id('input-mnemonic').typeText(' ')

  await id('input-password').tap()
  await id('input-password').replaceText(password)

  await id('input-chainType').tap()
  await id('input-chainType').replaceText(chainType)

  await id('input-network').tap()
  await id('input-network').replaceText(network)

  await id('input-segWit').tap()
  await id('input-segWit').replaceText(segWit)

  // dismiss keyboard
  // await label('return').tap()

  await id('import-btn').tap()

  await waitFor(id('import-address')).toExist().withTimeout(2000)

  await toHaveText('import-address', address)

  // go back
  await id('goBack').tap()
}
