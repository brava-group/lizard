# Pen.dev 1.2.21: movimento e idle do cursor do agente

Inspeção estática em 2026-11-04 de `/Applications/Pen.app/Contents/Resources/app.asar`
(SHA-256 `6020b5d71f5b701ef3a523f7e612a24d3d0ff18f151c5d5e705689ca65a6219a`).
O REA analisou apenas `out/editor/assets/index.js` e reteve o resultado em
`ev_2fcdaf2ec253dd64025e550605e3a306037456d0673d2607eb58fbeb9c87291b`;
a cobertura semântica do bundle foi parcial. A busca no grafo por
`AgentCursorMotion` não encontrou correspondência (`ev_25039857b19a2a995c2334cbd5b7ddddae92607973ee53a705aafa441a3b8e91`),
o que não prova ausência da função. Para responder à pergunta, o arquivo do editor
foi extraído como texto inerte para `/tmp/pen-current-editor.js` (SHA-256
`afacb701b827dbf5205596e50645d4840f3f08244e60ad486e88f700e3a102ff`).
Nenhuma sessão do Pen.dev, credencial ou documento do usuário foi executado ou lido.

## Observado no código empacotado

- O estado de movimento guarda posição atual, alvo, instante de aparição e última atividade.
  Atualizar o alvo preserva a posição atual enquanto o cursor ainda está visível. O render
  aproxima a posição do alvo com uma taxa que usa o tempo decorrido entre quadros; o alvo
  é convertido do documento para a tela apenas na pintura.
- Após cerca de 250 ms sem atividade, um deslocamento pequeno e periódico cresce de forma
  gradual. A preferência de animações desliga esse deslocamento. Ele é aplicado ao desenho
  do cursor, não altera a posição de trabalho nem o documento.
- Durante um turno em execução, a opacidade permanece plena. Depois da última atividade,
  o cursor espera cerca de cinco segundos e então desaparece ao longo de mais 1,5 segundo.
  O gerenciador remove cursores inativos depois desse período. Uma nova atividade renova
  a visibilidade; um cursor que já desapareceu pode reaparecer no novo alvo.
- Se o cursor ainda não tem alvo, a ativação inicial pode escolher uma posição próxima ao
  centro da câmera. Isso é uma posição inicial, não um destino após cada ferramenta.

Fonte estática: regiões do bundle que definem o estado de movimento `gte` e o gerenciador
`K1n`. Nomes minificados e parâmetros exatos são internos da versão inspecionada; não são
uma API nem foram copiados para o Lizard. A análise estática não comprova como cada fluxo
de agente utiliza esses métodos em runtime.

## Aplicação independente no Lizard

Manter a última posição em coordenadas da página entre ferramentas e variantes; aproximar
o cursor do alvo usando o tempo do frame; aplicar um idle discreto só ao desenho; manter o
cursor visível durante o turno e por um curto período após o fim. Um cursor sem histórico
aparece uma única vez no centro da área visível. Reduzir movimento desativa deslocamento e
aproximação animada. Nenhum desses efeitos entra no documento, Undo ou exportação.
